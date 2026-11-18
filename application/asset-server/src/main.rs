//! DodoGlass asset server: the catalogue of Movies and their Media files.

use std::io::{self, IsTerminal};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use asset_server::app::{App, Config};
use asset_server::ports::clock::SystemClock;
use asset_server::ports::mqtt::{MqttSettings, MqttUploadEvents};
use asset_server::ports::mqtt_status::{MqttStatusHints, MqttStatusSettings};
use asset_server::ports::storage::{Buckets, S3Settings};
use clap::{Parser, ValueEnum};
use tracing::info;
use tracing_subscriber::EnvFilter;

/// The DodoGlass asset server: places uploads in the catalogue and serves it over HTTP.
///
/// Every option can also be set with the environment variable shown next to it.
#[derive(Parser)]
#[command(name = "asset-server", version)]
struct Cli {
    /// SQLite database file (created, and migrated, at startup)
    #[arg(long, env = "ASSETS_DATABASE", default_value = "assets.db")]
    database: String,

    /// Address the HTTP API listens on. There is no authentication: keep it on localhost
    #[arg(long, env = "ASSETS_HTTP_ADDR", default_value = "127.0.0.1:8090")]
    http_addr: SocketAddr,

    /// MQTT broker the bucket events arrive through
    #[arg(long, env = "ASSETS_MQTT_URL", default_value = "mqtt://localhost:1883")]
    mqtt_url: String,

    /// Topic the bucket events of `ingest` arrive on
    #[arg(long, env = "ASSETS_MQTT_TOPIC", default_value = "dodoglass/ingest")]
    mqtt_topic: String,

    /// Shared-subscription group; must differ from the transcoders' group
    #[arg(long, env = "ASSETS_MQTT_GROUP", default_value = "assets")]
    mqtt_group: String,

    /// MQTT client id [default: assets-<hostname>-<pid>]; the status connection adds `-status`
    #[arg(long, env = "ASSETS_MQTT_CLIENT_ID")]
    mqtt_client_id: Option<String>,

    /// Topic the transcoders publish their status on (hints for an early reconcile)
    #[arg(
        long,
        env = "ASSETS_MQTT_STATUS_TOPIC",
        default_value = "dodoglass/status"
    )]
    mqtt_status_topic: String,

    /// S3 endpoint, e.g. http://localhost:9000
    #[arg(long, env = "ASSETS_S3_ENDPOINT")]
    s3_endpoint: String,

    /// S3 access key
    #[arg(long, env = "ASSETS_S3_ACCESS_KEY")]
    s3_access_key: String,

    /// S3 secret key (prefer the environment variable: arguments are visible in `ps`)
    #[arg(long, env = "ASSETS_S3_SECRET_KEY", hide_env_values = true)]
    s3_secret_key: String,

    /// S3 region
    #[arg(long, env = "ASSETS_S3_REGION", default_value = "us-east-1")]
    s3_region: String,

    /// Bucket videos are uploaded to
    #[arg(long, env = "ASSETS_INGEST_BUCKET", default_value = "ingest")]
    ingest_bucket: String,

    /// Bucket the transcoder writes its outputs to
    #[arg(long, env = "ASSETS_FINAL_BUCKET", default_value = "final")]
    final_bucket: String,

    /// Bucket the asset server keeps images in
    #[arg(long, env = "ASSETS_ASSETS_BUCKET", default_value = "assets")]
    assets_bucket: String,

    /// Seconds between two reconciles against the output markers in the final bucket
    #[arg(long, env = "ASSETS_RECONCILE_INTERVAL", default_value_t = 300,
          value_parser = clap::value_parser!(u64).range(1..))]
    reconcile_interval: u64,

    /// Log format: text for people, json for log collectors
    #[arg(long, env = "ASSETS_LOG_FORMAT", value_enum, default_value_t = LogFormat::Text)]
    log_format: LogFormat,
}

#[derive(Clone, Copy, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.log_format);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let buckets = Buckets::s3(
        &S3Settings {
            endpoint: cli.s3_endpoint.clone(),
            access_key: cli.s3_access_key,
            secret_key: cli.s3_secret_key,
            region: cli.s3_region,
        },
        &cli.ingest_bucket,
        &cli.final_bucket,
        &cli.assets_bucket,
    )?;
    buckets.check().await?;
    info!(endpoint = %cli.s3_endpoint, "buckets reachable");

    let config = Config {
        database: cli.database.clone(),
        final_bucket: cli.final_bucket,
    };
    let app = App::start(config, buckets, Arc::new(SystemClock)).await?;
    info!(database = %cli.database, "catalogue open, startup reconcile done");

    let client_id = cli.mqtt_client_id.unwrap_or_else(default_client_id);
    let events = MqttUploadEvents::connect(MqttSettings {
        broker: cli.mqtt_url.clone(),
        topic: cli.mqtt_topic,
        group: cli.mqtt_group,
        client_id: client_id.clone(),
    })
    .await?;
    app.spawn_ingest_listener(events);
    let hints = MqttStatusHints::connect(MqttStatusSettings {
        broker: cli.mqtt_url,
        topic: cli.mqtt_status_topic,
        client_id: format!("{client_id}-status"),
    })
    .await?;
    app.spawn_status_listener(hints);
    app.spawn_periodic_reconcile(Duration::from_secs(cli.reconcile_interval));

    let listener = tokio::net::TcpListener::bind(cli.http_addr)
        .await
        .with_context(|| format!("cannot listen on {}", cli.http_addr))?;
    info!(addr = %cli.http_addr, "HTTP API listening");
    axum::serve(listener, app.router())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("stopped");
    Ok(())
}

/// `assets-<hostname>-<pid>`: MQTT client ids must be unique per connection.
fn default_client_id() -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "host".into());
    format!("assets-{host}-{}", std::process::id())
}

fn init_logging(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(io::stdout);
    match format {
        LogFormat::Text => builder.with_ansi(io::stdout().is_terminal()).init(),
        LogFormat::Json => builder
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .init(),
    }
}

/// Waits for Ctrl+C or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("cannot listen for SIGTERM");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}
