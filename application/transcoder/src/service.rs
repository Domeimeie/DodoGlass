//! Unattended service mode: register on PubSub, take tasks, transcode them,
//! publish status. Logs go to stdout through `tracing`; there is no TUI.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::Result;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::config::Config;
use crate::ffmpeg::{self, Format, Video};
use crate::probe;
use crate::pubsub::{
    PubSub, Publisher, STATUS_CHANNEL, StatusEvent, StatusMessage, TASK_CHANNEL, Task, WorkerInfo,
    now_ms,
};
use crate::runner::{self, Event, Outcome};

/// How many of ffmpeg's last log lines to keep for the error message of a failed job.
const STDERR_TAIL: usize = 15;

pub struct Settings {
    pub worker_id: String,
    pub config: Config,
    pub out_dir: PathBuf,
    pub formats: Vec<Format>,
    /// How many tasks run at the same time.
    pub slots: usize,
}

/// Shared by every task handler.
struct Context {
    settings: Settings,
    cancel: AtomicBool,
}

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
        tasks = TASK_CHANNEL,
        status = STATUS_CHANNEL,
        "worker registered, waiting for tasks"
    );

    let slots = Arc::new(Semaphore::new(settings.slots));
    let publisher = pubsub.publisher();
    let ctx = Arc::new(Context {
        settings,
        cancel: AtomicBool::new(false),
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
    info!(input = %task.input.display(), "task accepted");
    publish(status(StatusEvent::Accepted)).await;

    let input = task.input.clone();
    let source = match tokio::task::spawn_blocking(move || probe::probe(&input)).await {
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
        path: task.input.clone(),
        name: task.id.clone(),
        out_dir: s.out_dir.join(&task.id),
        source: Ok(source.clone()),
    };
    let formats = task.formats.clone().unwrap_or_else(|| s.formats.clone());
    let jobs = ffmpeg::build_jobs(0, &video, &source, &s.config, &formats);

    for (index, job) in jobs.iter().enumerate() {
        let label = job.format.label();
        if ctx.cancel.load(Ordering::Relaxed) {
            let mut m = status(StatusEvent::Failed);
            m.format = Some(label);
            return fail(&publish, m, "cancelled before starting".into()).await;
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

    info!(
        took_s = format!("{:.1}", started.elapsed().as_secs_f64()),
        output = %video.out_dir.display(),
        "task completed"
    );
    publish(status(StatusEvent::Completed)).await;
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
