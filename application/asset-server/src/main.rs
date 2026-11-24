//! DodoGlass asset server: the catalogue of Movies and Shows (Seasons,
//! Episodes) and their Media files.

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
use asset_server::ports::tmdb::{NoTmdb, Tmdb, TmdbClient, TmdbSettings};
use clap::{Parser, ValueEnum};
use tracing::{info, warn};
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

    /// TMDB API key (v3) or read access token (v4). Without one, Movies and Shows are
    /// not looked up and keep the name and year from their folder
    #[arg(long, env = "ASSETS_TMDB_API_KEY", hide_env_values = true)]
    tmdb_api_key: Option<String>,

    /// Language of the names and descriptions taken from TMDB
    #[arg(long, env = "ASSETS_TMDB_LANGUAGE", default_value = "en-US")]
    tmdb_language: String,

    /// Seconds between two passes looking new Movies, Shows, Seasons and Episodes (and
    /// due retries) up in TMDB
    #[arg(long, env = "ASSETS_TMDB_INTERVAL", default_value_t = 10,
          value_parser = clap::value_parser!(u64).range(1..))]
    tmdb_interval: u64,

    /// Ratings (TMDB's user scores) older than this are fetched again by the rating sweep,
    /// e.g. 7d, 36h
    #[arg(long, env = "ASSETS_RATING_MAX_AGE", default_value = "7d",
          value_parser = positive_duration)]
    rating_max_age: Duration,

    /// Time between two rating sweeps, e.g. 1h, 30m
    #[arg(long, env = "ASSETS_RATING_SWEEP_INTERVAL", default_value = "1h",
          value_parser = positive_duration)]
    rating_sweep_interval: Duration,

    /// Log format: text for people, json for log collectors
    #[arg(long, env = "ASSETS_LOG_FORMAT", value_enum, default_value_t = LogFormat::Text)]
    log_format: LogFormat,
}

/// A duration such as `7d`, `1h 30m` or `90s`; not zero.
fn positive_duration(s: &str) -> Result<Duration, String> {
    match humantime::parse_duration(s) {
        Ok(d) if d.is_zero() => Err("must be longer than zero".into()),
        Ok(d) => Ok(d),
        Err(e) => Err(format!("{e} (e.g. 7d, 1h, 30m)")),
    }
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
        rating_max_age: cli.rating_max_age,
        ..Config::default()
    };
    let tmdb: Arc<dyn Tmdb> = match &cli.tmdb_api_key {
        Some(key) => Arc::new(TmdbClient::new(TmdbSettings::new(key, &cli.tmdb_language))?),
        None => Arc::new(NoTmdb),
    };
    let app = App::start(config, buckets, Arc::new(SystemClock), tmdb).await?;
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
    if cli.tmdb_api_key.is_some() {
        app.spawn_periodic_matcher(Duration::from_secs(cli.tmdb_interval));
        app.spawn_periodic_rating_sweep(cli.rating_sweep_interval);
        info!(
            language = %cli.tmdb_language,
            rating_max_age = %humantime::format_duration(cli.rating_max_age),
            rating_sweep_interval = %humantime::format_duration(cli.rating_sweep_interval),
            "TMDB lookups and rating sweep on"
        );
    } else {
        warn!("no TMDB API key (--tmdb-api-key): Movies and Shows are not looked up");
    }

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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    /// The settings every start needs.
    const REQUIRED: [&str; 7] = [
        "asset-server",
        "--s3-endpoint",
        "http://localhost:9000",
        "--s3-access-key",
        "key",
        "--s3-secret-key",
        "secret",
    ];

    /// The environment variable and default of setting `id`.
    fn env_and_default(id: &str) -> (String, String) {
        let command = Cli::command();
        let arg = command
            .get_arguments()
            .find(|a| a.get_id() == id)
            .unwrap_or_else(|| panic!("no setting {id}"));
        let env = arg.get_env().expect("an environment variable");
        let default = arg.get_default_values();
        (
            env.to_string_lossy().into(),
            default[0].to_string_lossy().into(),
        )
    }

    #[test]
    fn the_rating_settings_have_environment_variables_and_defaults() {
        assert_eq!(
            env_and_default("rating_max_age"),
            ("ASSETS_RATING_MAX_AGE".into(), "7d".into())
        );
        assert_eq!(
            env_and_default("rating_sweep_interval"),
            ("ASSETS_RATING_SWEEP_INTERVAL".into(), "1h".into())
        );
        assert_eq!(
            positive_duration("7d"),
            Ok(asset_server::matcher::RATING_MAX_AGE)
        );
        assert_eq!(
            positive_duration("1h"),
            Ok(asset_server::matcher::RATING_SWEEP_INTERVAL)
        );
    }

    #[test]
    fn the_rating_settings_can_be_given_as_flags() {
        let mut args = REQUIRED.to_vec();
        args.extend(["--rating-max-age", "36h", "--rating-sweep-interval", "15m"]);
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.rating_max_age, Duration::from_secs(36 * 3600));
        assert_eq!(cli.rating_sweep_interval, Duration::from_secs(15 * 60));
    }

    #[test]
    fn the_rating_settings_can_be_given_as_environment_variables() {
        // SAFETY: no other test of this binary reads these two variables
        // without giving the setting as a flag (which wins over them).
        unsafe {
            std::env::set_var("ASSETS_RATING_MAX_AGE", "3d");
            std::env::set_var("ASSETS_RATING_SWEEP_INTERVAL", "2h");
        }
        let cli = Cli::try_parse_from(REQUIRED);
        unsafe {
            std::env::remove_var("ASSETS_RATING_MAX_AGE");
            std::env::remove_var("ASSETS_RATING_SWEEP_INTERVAL");
        }
        let cli = cli.unwrap();
        assert_eq!(cli.rating_max_age, Duration::from_secs(3 * 24 * 3600));
        assert_eq!(cli.rating_sweep_interval, Duration::from_secs(2 * 3600));
    }

    #[test]
    fn a_rating_setting_must_be_a_duration_longer_than_zero() {
        for bad in ["0s", "soon", "7"] {
            let mut args = REQUIRED.to_vec();
            args.extend(["--rating-max-age", bad]);
            assert!(Cli::try_parse_from(args).is_err(), "{bad}");
        }
    }
}
