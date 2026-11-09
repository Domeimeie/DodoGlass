//! DodoGlass transcoder: an ffmpeg wrapper that turns videos into
//! HLS and DASH with several quality levels.

mod config;
mod ffmpeg;
mod headless;
mod probe;
mod pubsub;
mod runner;
mod service;
mod storage;
mod tui;
mod upload;

use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};

use crate::config::{Config, DEFAULT_CONFIG};
use crate::ffmpeg::{Format, OutDir, Video};
use crate::pubsub::{VIDEO_EXTENSIONS, is_video_name};

/// Transcode videos into HLS and DASH with several quality levels (ffmpeg wrapper).
///
/// Every option can also be set with the environment variable shown next to it,
/// which is how the service is configured in a container.
#[derive(Parser)]
#[command(name = "transcoder", version)]
struct Cli {
    /// A video file, or a folder: every .mp4, .mov and .mkv file directly inside it is transcoded
    #[arg(required_unless_present_any = ["print_config", "service"])]
    input: Option<PathBuf>,

    /// Transcoding config (TOML). Uses the built-in ladder if omitted
    #[arg(short, long, env = "TRANSCODER_CONFIG")]
    config: Option<PathBuf>,

    /// Output folder. Each video gets its own subfolder: <OUT>/<video name or task id>
    #[arg(short, long, env = "TRANSCODER_OUT", default_value = "out")]
    out: PathBuf,

    /// Which formats to produce (in service mode: for tasks that don't say)
    #[arg(short, long, env = "TRANSCODER_FORMAT", value_enum, default_value_t = FormatArg::Both)]
    format: FormatArg,

    /// How many transcodes run at the same time (in service mode: tasks)
    #[arg(short, long, env = "TRANSCODER_THREADS", default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..))]
    threads: u16,

    /// Plain text output instead of the TUI (automatic when not run in a terminal)
    #[arg(long)]
    headless: bool,

    /// Print the ffmpeg commands without running them
    #[arg(long)]
    dry_run: bool,

    /// Print the built-in config (a starting point for your own) and exit
    #[arg(long)]
    print_config: bool,

    /// Run as an unattended service: take tasks from PubSub instead of INPUT
    #[arg(long, env = "TRANSCODER_SERVICE", conflicts_with_all = ["input", "headless", "dry_run"])]
    service: bool,

    /// PubSub backend for the service
    #[arg(long, env = "TRANSCODER_PUBSUB", value_enum, default_value_t = PubSubKind::Mock)]
    pubsub: PubSubKind,

    /// Folder the mock PubSub keeps its queues in
    #[arg(long, env = "TRANSCODER_MOCK_DIR", default_value = "mock-pubsub")]
    mock_dir: PathBuf,

    /// MQTT broker (with --pubsub mqtt)
    #[arg(long, env = "TRANSCODER_MQTT_URL", default_value = "mqtt://localhost:1883")]
    mqtt_url: String,

    /// Topic the bucket events arrive on
    #[arg(long, env = "TRANSCODER_MQTT_TOPIC", default_value = "dodoglass/ingest")]
    mqtt_topic: String,

    /// Shared-subscription group: workers in one group split the events between them
    #[arg(long, env = "TRANSCODER_MQTT_GROUP", default_value = "transcoders")]
    mqtt_group: String,

    /// Topic status messages are published to
    #[arg(long, env = "TRANSCODER_MQTT_STATUS_TOPIC", default_value = "dodoglass/status")]
    mqtt_status_topic: String,

    /// Workers register (retained) below this topic
    #[arg(long, env = "TRANSCODER_MQTT_WORKERS_TOPIC", default_value = "dodoglass/workers")]
    mqtt_workers_topic: String,

    /// S3 endpoint for inputs in buckets and for --output-bucket, e.g. http://localhost:9000
    #[arg(long, env = "TRANSCODER_S3_ENDPOINT")]
    s3_endpoint: Option<String>,

    /// S3 access key
    #[arg(long, env = "TRANSCODER_S3_ACCESS_KEY", requires = "s3_endpoint")]
    s3_access_key: Option<String>,

    /// S3 secret key (prefer the environment variable: arguments are visible in `ps`)
    #[arg(long, env = "TRANSCODER_S3_SECRET_KEY", hide_env_values = true, requires = "s3_endpoint")]
    s3_secret_key: Option<String>,

    /// S3 region
    #[arg(long, env = "TRANSCODER_S3_REGION", default_value = "us-east-1")]
    s3_region: String,

    /// Upload results to this bucket instead of writing them to --out
    #[arg(long, env = "TRANSCODER_OUTPUT_BUCKET", requires = "s3_endpoint")]
    output_bucket: Option<String>,

    /// Name this worker registers with [default: <hostname>-<pid>]
    #[arg(long, env = "TRANSCODER_WORKER_ID")]
    worker_id: Option<String>,

    /// Service log format: text for people, json for log collectors
    #[arg(long, env = "TRANSCODER_LOG_FORMAT", value_enum, default_value_t = LogFormat::Text)]
    log_format: LogFormat,
}

#[derive(Clone, Copy, ValueEnum)]
enum PubSubKind {
    /// Files in --mock-dir (for development)
    Mock,
    /// An MQTT broker that receives bucket events (--mqtt-*)
    Mqtt,
}

#[derive(Clone, Copy, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Hls,
    Dash,
    Both,
}

impl FormatArg {
    fn formats(self) -> Vec<Format> {
        match self {
            FormatArg::Hls => vec![Format::Hls],
            FormatArg::Dash => vec![Format::Dash],
            FormatArg::Both => vec![Format::Hls, Format::Dash],
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool> {
    let cli = Cli::parse();
    if cli.print_config {
        print!("{DEFAULT_CONFIG}");
        return Ok(true);
    }
    if cli.service {
        return run_service(cli);
    }
    let input = cli.input.context("no input file given")?;
    let cfg = Config::load(cli.config.as_deref())?;
    let paths = discover(&input)?;
    let threads = cli.threads as usize;

    // ffmpeg processes are started and watched by tasks on this runtime.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()
        .context("cannot start the tokio runtime")?;

    let sources = runtime.block_on(probe_all(paths.clone()));
    if input.is_file()
        && let Err(e) = &sources[0]
    {
        bail!("{e}");
    }
    let videos: Vec<Video> = paths
        .into_iter()
        .zip(sources)
        .map(|(path, source)| {
            let name = file_stem(&path);
            Video {
                out_dir: OutDir::Local(cli.out.join(&name)),
                input: path.display().to_string(),
                input_etag: None,
                name,
                source,
            }
        })
        .collect();

    let formats = cli.format.formats();
    let mut jobs = Vec::new();
    let mut warnings = Vec::new();
    for (i, video) in videos.iter().enumerate() {
        let Ok(src) = &video.source else { continue };
        jobs.extend(ffmpeg::build_jobs(i, video, src, &cfg, &formats));
        for r in cfg.renditions.iter().filter(|r| r.height > src.height) {
            warnings.push(format!(
                "{}: rendition {} ({}p) is taller than the source ({}p) and will be upscaled",
                video.name, r.name, r.height, src.height
            ));
        }
        for r in &cfg.renditions {
            if let Some(fps) = cfg.fps_for(r)
                && src.fps > 0.0
                && fps > src.fps + 0.01
            {
                warnings.push(format!(
                    "{}: rendition {} ({fps} fps) has a higher frame rate than the source ({:.3} fps); frames will be duplicated",
                    video.name, r.name, src.fps
                ));
            }
        }
    }

    if cli.dry_run {
        for w in &warnings {
            eprintln!("warning: {w}");
        }
        for v in &videos {
            if let Err(e) = &v.source {
                eprintln!("✘ {}: {e}", v.name);
            }
        }
        for job in &jobs {
            println!(
                "# {} {} → {}\n{}\n",
                videos[job.video].name,
                job.format.label(),
                job.manifest.display(),
                job.command_line()
            );
        }
        return Ok(true);
    }

    // The jobs run on the tokio runtime and report back over a channel,
    // so the TUI (or headless printer) on this thread stays responsive.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker = runtime.spawn(runner::run_all(jobs.clone(), threads, tx, cancel.clone()));

    let headless = cli.headless || !io::stdout().is_terminal();
    let ok = if headless {
        headless::run(&videos, &jobs, threads, &warnings, rx)
    } else {
        let app = tui::App::new(input, threads, cfg, videos, jobs, warnings, cancel);
        tui::run(app, rx)?
    };
    let _ = runtime.block_on(worker);
    Ok(ok)
}

/// The input itself, or every video file (.mp4, .mov, .mkv) directly inside it if it is a folder.
fn discover(input: &Path) -> Result<Vec<PathBuf>> {
    if input.is_file() {
        return Ok(vec![input.to_path_buf()]);
    }
    if !input.is_dir() {
        bail!("{} does not exist", input.display());
    }
    let mut files: Vec<PathBuf> = fs::read_dir(input)
        .with_context(|| format!("cannot read folder {}", input.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && is_video_name(&p.to_string_lossy()))
        .collect();
    files.sort();
    if files.is_empty() {
        bail!(
            "no video files (.{}) in {}",
            VIDEO_EXTENSIONS.join(", ."),
            input.display()
        );
    }
    // Each video writes to <out>/<name>, so names must not repeat (a.mp4 vs a.MKV).
    let mut names = HashSet::new();
    for f in &files {
        if !names.insert(file_stem(f).to_lowercase()) {
            bail!(
                "two files would write to the same output folder: {}",
                file_stem(f)
            );
        }
    }
    Ok(files)
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into())
}

/// Probes all files at once; ffprobe is quick but a folder can hold many files.
async fn probe_all(paths: Vec<PathBuf>) -> Vec<Result<probe::SourceInfo, String>> {
    let handles: Vec<_> = paths
        .into_iter()
        .map(|p| {
            tokio::task::spawn_blocking(move || {
                probe::probe(&p.display().to_string(), None).map_err(|e| format!("{e:#}"))
            })
        })
        .collect();
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        results.push(handle.await.unwrap_or_else(|e| Err(e.to_string())));
    }
    results
}

fn run_service(cli: Cli) -> Result<bool> {
    init_logging(cli.log_format);
    let config = Config::load(cli.config.as_deref())?;
    let slots = cli.threads as usize;
    let worker_id = cli.worker_id.clone().unwrap_or_else(service::default_worker_id);

    let storage = match &cli.s3_endpoint {
        Some(endpoint) => Some(Arc::new(storage::Storage::new(storage::S3Settings {
            endpoint: endpoint.clone(),
            access_key: cli
                .s3_access_key
                .clone()
                .context("--s3-access-key (TRANSCODER_S3_ACCESS_KEY) is missing")?,
            secret_key: cli
                .s3_secret_key
                .clone()
                .context("--s3-secret-key (TRANSCODER_S3_SECRET_KEY) is missing")?,
            region: cli.s3_region.clone(),
        }))),
        None => None,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(slots)
        .enable_all()
        .build()
        .context("cannot start the tokio runtime")?;
    runtime.block_on(async {
        let output = match (&cli.output_bucket, &storage) {
            (Some(bucket), Some(storage)) => {
                // Fail now rather than after the first transcode.
                storage.check_bucket(bucket).await?;
                let uploads = upload::UploadServer::start(storage.clone(), bucket.clone()).await?;
                tracing::info!(bucket = %bucket, endpoint = storage.endpoint(), "results go to bucket");
                service::Output::Bucket(uploads)
            }
            _ => service::Output::Local(cli.out.clone()),
        };
        let settings = service::Settings {
            worker_id: worker_id.clone(),
            config,
            output,
            storage,
            formats: cli.format.formats(),
            slots,
        };
        match cli.pubsub {
            PubSubKind::Mock => {
                let pubsub = pubsub::mock::MockPubSub::new(cli.mock_dir, &worker_id);
                service::run(pubsub, settings).await
            }
            PubSubKind::Mqtt => {
                let mqtt = pubsub::mqtt::MqttSettings {
                    broker: cli.mqtt_url,
                    topic: cli.mqtt_topic,
                    group: cli.mqtt_group,
                    status_topic: cli.mqtt_status_topic,
                    workers_topic: cli.mqtt_workers_topic,
                };
                let pubsub = pubsub::mqtt::MqttPubSub::new(mqtt, &worker_id, slots)?;
                service::run(pubsub, settings).await
            }
        }
    })?;
    Ok(true)
}

/// Service logs: one line per event on stdout. Level filter from RUST_LOG
/// (default: info; RUST_LOG=debug also shows ffmpeg's own output).
fn init_logging(format: LogFormat) {
    use tracing_subscriber::EnvFilter;
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
