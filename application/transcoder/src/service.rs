//! Unattended service mode: register on PubSub, take tasks, transcode them,
//! publish status. Logs go to stdout through `tracing`; there is no TUI.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::config::Config;
use crate::ffmpeg::{self, Format, OutDir, Video};
use crate::probe;
use crate::pubsub::{
    Input, PubSub, Publisher, StatusEvent, StatusMessage, Task, WorkerInfo, now_ms,
};
use crate::runner::{self, Event, Outcome};
use crate::storage::{Removal, Storage};
use crate::upload::UploadServer;

/// How many of ffmpeg's last log lines to keep for the error message of a failed job.
const STDERR_TAIL: usize = 15;

pub struct Settings {
    pub worker_id: String,
    pub config: Config,
    pub output: Output,
    /// Needed for tasks whose input is an object in a bucket.
    pub storage: Option<Arc<Storage>>,
    pub formats: Vec<Format>,
    /// How many tasks run at the same time.
    pub slots: usize,
    /// Keep inputs in their bucket after a completed or skipped task
    /// (default: delete them, so the input bucket only holds what is waiting).
    pub keep_input: bool,
}

/// Where results go.
pub enum Output {
    /// `<dir>/<task id>/`
    Local(PathBuf),
    /// `s3://<bucket>/<task id>/`, uploaded file by file while ffmpeg runs.
    Bucket(UploadServer),
}

impl Output {
    fn dir_for(&self, task_id: &str) -> OutDir {
        match self {
            Output::Local(dir) => OutDir::Local(dir.join(task_id)),
            Output::Bucket(uploads) => OutDir::Http {
                url: uploads.url_for(task_id),
                shown: format!("s3://{}/{task_id}", uploads.bucket()),
            },
        }
    }
}

/// Shared by every task handler.
struct Context {
    settings: Settings,
    cancel: AtomicBool,
    /// One lock per task id (= output folder): a second task for the same
    /// video waits until the first is finished instead of overwriting it.
    running_ids: std::sync::Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl Context {
    fn lock_for(&self, id: &str) -> Arc<AsyncMutex<()>> {
        let mut ids = self.running_ids.lock().expect("lock poisoned");
        // Forget locks nobody holds any more, so the map doesn't grow forever.
        ids.retain(|_, lock| Arc::strong_count(lock) > 1);
        ids.entry(id.to_string()).or_default().clone()
    }
}

/// Written to `<output>/<task id>/source.json` after a task completed: which
/// version of which input the output was made from. A new event for the same
/// version (same eTag) and formats is then skipped.
#[derive(Debug, Serialize, Deserialize)]
struct SourceMarker {
    input: String,
    etag: String,
    formats: Vec<Format>,
    completed_ms: u64,
    worker: String,
}

const SOURCE_MARKER: &str = "source.json";

/// Runs until SIGTERM / Ctrl+C. The first signal stops taking tasks and lets
/// running ones finish; a second one cancels them.
pub async fn run<P: PubSub>(mut pubsub: P, settings: Settings) -> Result<()> {
    let worker = WorkerInfo {
        id: settings.worker_id.clone(),
        pid: std::process::id(),
        slots: settings.slots,
        started_ms: now_ms(),
        version: env!("CARGO_PKG_VERSION"),
    };
    pubsub.register(&worker).await?;
    info!(
        worker = %worker.id,
        slots = worker.slots,
        "worker registered, waiting for tasks"
    );

    let slots = Arc::new(Semaphore::new(settings.slots));
    let publisher = pubsub.publisher();
    let ctx = Arc::new(Context {
        settings,
        cancel: AtomicBool::new(false),
        running_ids: Default::default(),
    });
    let mut running = JoinSet::new();
    let mut shutdown = pin!(shutdown_signal());

    loop {
        // Only take a task when a slot is free, so tasks this worker can't
        // start yet stay available for other workers.
        let permit = tokio::select! {
            permit = slots.clone().acquire_owned() => permit?,
            _ = &mut shutdown => break,
        };
        let task = tokio::select! {
            task = pubsub.next_task() => task,
            _ = &mut shutdown => break,
        };
        match task {
            Ok(task) => {
                let span = info_span!("task", id = %task.id);
                running.spawn(
                    handle_task(task, permit, publisher.clone(), ctx.clone()).instrument(span),
                );
            }
            Err(e) => error!(error = format!("{e:#}"), "could not read from PubSub"),
        }
        // Collect finished handlers so the set doesn't grow forever.
        while running.try_join_next().is_some() {}
    }

    if !running.is_empty() {
        info!(
            running = running.len(),
            "shutting down: finishing running tasks (signal again to cancel them)"
        );
        tokio::select! {
            _ = drain(&mut running) => {}
            _ = shutdown_signal() => {
                warn!("cancelling running tasks");
                ctx.cancel.store(true, Ordering::Relaxed);
                drain(&mut running).await;
            }
        }
    }
    pubsub.unregister().await?;
    info!(worker = %ctx.settings.worker_id, "worker stopped");
    Ok(())
}

async fn drain(running: &mut JoinSet<()>) {
    while running.join_next().await.is_some() {}
}

/// Resolves on Ctrl+C (SIGINT) or SIGTERM, which is what `docker stop` sends.
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

/// Probes the input, then runs its formats one after another. Holds one slot
/// (`_permit`) for the whole task.
async fn handle_task<P: Publisher + Clone>(
    task: Task,
    _permit: OwnedSemaphorePermit,
    publisher: P,
    ctx: Arc<Context>,
) {
    let s = &ctx.settings;
    let status = |event| StatusMessage::new(&s.worker_id, &task.id, event);
    let publish = |message: StatusMessage| {
        let publisher = publisher.clone();
        async move {
            if let Err(e) = publisher.publish(&message).await {
                error!(error = format!("{e:#}"), "could not publish status");
            }
        }
    };
    let started = Instant::now();
    info!(input = %task.input, "task accepted");
    publish(status(StatusEvent::Accepted)).await;

    // The same video (task id) at most once at a time. Waiting keeps the slot.
    let id_lock = ctx.lock_for(&task.id);
    let _same_id = match id_lock.clone().try_lock_owned() {
        Ok(guard) => guard,
        Err(_) => {
            info!("waiting: this video is already being transcoded");
            id_lock.lock_owned().await
        }
    };
    let formats = task.formats.clone().unwrap_or_else(|| s.formats.clone());
    let marker_key = format!("{}/{SOURCE_MARKER}", task.id);

    // Unchanged re-upload (same eTag, same formats): nothing to do.
    if let (
        Output::Bucket(uploads),
        Some(storage),
        Input::Object {
            etag: Some(etag), ..
        },
    ) = (&s.output, &s.storage, &task.input)
    {
        match storage.get_small(uploads.bucket(), &marker_key).await {
            Ok(Some(json)) => match serde_json::from_slice::<SourceMarker>(&json) {
                Ok(m) if m.etag == *etag && m.formats == formats => {
                    info!(%etag, "unchanged since the last transcode, skipped");
                    consume_input(&task, s).await;
                    return publish(status(StatusEvent::Skipped)).await;
                }
                Ok(_) => info!("new version of this video: replacing the earlier output"),
                Err(e) => warn!(error = %e, "ignoring unreadable {SOURCE_MARKER}"),
            },
            Ok(None) => {}
            Err(e) => warn!(error = format!("{e:#}"), "cannot read {SOURCE_MARKER}"),
        }
    }

    // What ffmpeg opens. For an object: a presigned URL, so ffmpeg streams the
    // video straight from the storage; nothing is downloaded first.
    let input = match &task.input {
        Input::File(path) => path.display().to_string(),
        Input::Object { bucket, key, .. } => {
            let Some(storage) = &s.storage else {
                let reason = "the input is in a bucket, but no S3 storage is configured".into();
                return fail(&publish, status(StatusEvent::Failed), reason).await;
            };
            match storage.read_url(bucket, key).await {
                Ok(url) => url,
                Err(e) => {
                    return fail(&publish, status(StatusEvent::Failed), format!("{e:#}")).await;
                }
            }
        }
    };

    let input_etag = match &task.input {
        Input::Object { etag, .. } => etag.clone(),
        Input::File(_) => None,
    };
    let (probe_input, probe_etag) = (input.clone(), input_etag.clone());
    let probed =
        tokio::task::spawn_blocking(move || probe::probe(&probe_input, probe_etag.as_deref()));
    let source = match probed.await {
        Ok(Ok(source)) => source,
        Ok(Err(e)) => return fail(&publish, status(StatusEvent::Failed), format!("{e:#}")).await,
        Err(e) => return fail(&publish, status(StatusEvent::Failed), e.to_string()).await,
    };
    info!(
        width = source.width,
        height = source.height,
        fps = format!("{:.3}", source.fps),
        duration_s = format!("{:.1}", source.duration),
        audio = source.has_audio(),
        "input probed"
    );

    let video = Video {
        input,
        input_etag: input_etag.clone(),
        name: task.id.clone(),
        out_dir: s.output.dir_for(&task.id),
        source: Ok(source.clone()),
    };
    let jobs = ffmpeg::build_jobs(0, &video, &source, &s.config, &formats);

    // From here on the earlier output is replaced: it no longer matches the marker.
    if let (Output::Bucket(uploads), Some(storage)) = (&s.output, &s.storage)
        && let Err(e) = storage.delete(uploads.bucket(), &marker_key).await
    {
        return fail(&publish, status(StatusEvent::Failed), format!("{e:#}")).await;
    }

    for (index, job) in jobs.iter().enumerate() {
        let label = job.format.label();
        if ctx.cancel.load(Ordering::Relaxed) {
            let mut m = status(StatusEvent::Failed);
            m.format = Some(label);
            return fail(&publish, m, "cancelled before starting".into()).await;
        }
        // In a bucket, remove an earlier run's output first (on disk the runner does it).
        let prefix = format!("{}/{}", task.id, job.format.dir_name());
        if let (Output::Bucket(uploads), Some(storage)) = (&s.output, &s.storage) {
            match storage.delete_prefix(uploads.bucket(), &prefix).await {
                Ok(0) => {}
                Ok(n) => info!(format = label, deleted = n, "removed earlier output"),
                Err(e) => {
                    let mut m = status(StatusEvent::Failed);
                    m.format = Some(label);
                    return fail(&publish, m, format!("{e:#}")).await;
                }
            }
            uploads.take_stats(&prefix);
        }
        info!(format = label, "job started");
        let mut m = status(StatusEvent::JobStarted);
        m.format = Some(label);
        publish(m).await;

        // Run ffmpeg while turning its events into logs and status messages.
        let job_started = Instant::now();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut run = pin!(runner::run_job(index, job, &tx, &ctx.cancel, true));
        let mut tail = VecDeque::with_capacity(STDERR_TAIL);
        let mut next_quarter = 25.0;
        let outcome = loop {
            tokio::select! {
                outcome = &mut run => break outcome,
                Some(event) = rx.recv() => match event {
                    Event::Log { line, .. } => {
                        debug!(target: "ffmpeg", format = label, "{line}");
                        if tail.len() == STDERR_TAIL {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                    Event::Progress { seconds, speed, .. } => {
                        let percent = if source.duration > 0.0 {
                            (seconds / source.duration * 100.0).clamp(0.0, 100.0)
                        } else {
                            0.0
                        };
                        // Log and publish at 25 %, 50 %, 75 % rather than twice a second.
                        if percent >= next_quarter && next_quarter < 100.0 {
                            while percent >= next_quarter {
                                next_quarter += 25.0;
                            }
                            let speed = speed.map(|s| format!("{s:.2}x")).unwrap_or_else(|| "-".into());
                            info!(format = label, percent = format!("{percent:.0}"), speed, "progress");
                            let mut m = status(StatusEvent::Progress);
                            m.format = Some(label);
                            m.percent = Some((percent * 10.0).round() / 10.0);
                            publish(m).await;
                        }
                    }
                    _ => {}
                },
            }
        };

        let took_s = format!("{:.1}", job_started.elapsed().as_secs_f64());
        // ffmpeg does not always stop when an upload fails, so check.
        let mut outcome = outcome;
        if let Output::Bucket(uploads) = &s.output {
            let stats = uploads.take_stats(&prefix);
            if outcome == Outcome::Done && stats.failed > 0 {
                outcome = Outcome::Failed(format!(
                    "{} of {} uploads failed, last: {}",
                    stats.failed,
                    stats.failed + stats.files,
                    stats.last_error.unwrap_or_default()
                ));
            } else if outcome == Outcome::Done {
                info!(
                    format = label,
                    files = stats.files,
                    uploaded_mb = format!("{:.1}", stats.bytes as f64 / 1e6),
                    "uploaded"
                );
            }
        }
        match outcome {
            Outcome::Done => {
                info!(format = label, took_s, manifest = %job.manifest.display(), "job done");
                let mut m = status(StatusEvent::JobDone);
                m.format = Some(label);
                m.manifest = Some(job.manifest.display().to_string());
                publish(m).await;
            }
            Outcome::Failed(reason) => {
                // The last ffmpeg lines usually say why; include them once, here.
                let ffmpeg_output = tail.into_iter().collect::<Vec<_>>().join("\n");
                error!(format = label, took_s, %reason, ffmpeg_output, "job failed");
                let mut m = status(StatusEvent::JobFailed);
                m.format = Some(label);
                m.error = Some(reason.clone());
                publish(m).await;
                let mut m = status(StatusEvent::Failed);
                m.format = Some(label);
                return fail(&publish, m, reason).await;
            }
            Outcome::Cancelled => {
                let mut m = status(StatusEvent::Failed);
                m.format = Some(label);
                return fail(&publish, m, "cancelled".into()).await;
            }
        }
    }

    if let (Output::Bucket(uploads), Some(storage), Some(etag)) =
        (&s.output, &s.storage, input_etag)
    {
        let marker = SourceMarker {
            input: task.input.to_string(),
            etag,
            formats,
            completed_ms: now_ms(),
            worker: s.worker_id.clone(),
        };
        let json = serde_json::to_vec_pretty(&marker).unwrap_or_default();
        if let Err(e) = storage
            .put(uploads.bucket(), &marker_key, json.into())
            .await
        {
            // The output is complete; only the next identical upload won't be skipped.
            warn!(error = format!("{e:#}"), "cannot write {SOURCE_MARKER}");
        }
    }
    consume_input(&task, s).await;
    info!(
        took_s = format!("{:.1}", started.elapsed().as_secs_f64()),
        output = %video.out_dir.display(),
        "task completed"
    );
    publish(status(StatusEvent::Completed)).await;
}

/// After a completed or skipped task: delete the input object, but only the
/// version that was transcoded. A failed task keeps its input for a retry;
/// files on disk are never deleted.
async fn consume_input(task: &Task, s: &Settings) {
    if s.keep_input {
        return;
    }
    let (Input::Object { bucket, key, etag }, Some(storage)) = (&task.input, &s.storage) else {
        return;
    };
    let Some(etag) = etag else {
        warn!("input kept: the event had no eTag, so its version cannot be checked");
        return;
    };
    match storage.delete_if_unchanged(bucket, key, etag).await {
        Ok(Removal::Deleted) => info!(input = %task.input, "input removed"),
        Ok(Removal::Changed) => info!(
            input = %task.input,
            "input kept: a newer upload with the same name arrived meanwhile"
        ),
        Ok(Removal::Gone) => debug!(input = %task.input, "input was already removed"),
        Err(e) => warn!(error = format!("{e:#}"), "could not remove the input"),
    }
}

async fn fail<F, Fut>(publish: &F, mut message: StatusMessage, reason: String)
where
    F: Fn(StatusMessage) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    warn!(%reason, "task failed");
    message.error = Some(reason);
    publish(message).await;
}

/// `<hostname>-<pid>`. Inside a container the hostname is the container ID.
pub fn default_worker_id() -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "worker".into());
    format!("{host}-{}", std::process::id())
}
