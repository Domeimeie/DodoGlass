//! Runs the jobs on the tokio runtime, at most `parallel` ffmpeg processes at a time.

use std::fs;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedSender as Sender;
use tokio::task::JoinSet;

use crate::ffmpeg::{Job, OutDir, redact};

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Failed(String),
    Cancelled,
}

/// Messages from the jobs to whoever shows progress (TUI or headless).
/// `job` is always an index into the job list.
pub enum Event {
    JobStarted(usize),
    Progress {
        job: usize,
        seconds: f64,
        speed: Option<f64>,
    },
    Log {
        job: usize,
        line: String,
    },
    JobFinished {
        job: usize,
        outcome: Outcome,
    },
    AllDone,
}

/// Starts every job as a tokio task. A semaphore with `parallel` permits makes
/// all but `parallel` of them wait, in order. A failing job does not stop the
/// others; setting `cancel` does.
pub async fn run_all(jobs: Vec<Job>, parallel: usize, tx: Sender<Event>, cancel: Arc<AtomicBool>) {
    let permits = Arc::new(Semaphore::new(parallel));
    let mut tasks = JoinSet::new();

    for (index, job) in jobs.into_iter().enumerate() {
        // Taking the permit here, before spawning, starts the jobs in list order.
        // (Tasks spawned first don't necessarily run first on a multi-threaded runtime.)
        let permit = permits
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore is never closed");
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Event::JobFinished {
                job: index,
                outcome: Outcome::Cancelled,
            });
            continue;
        }
        let _ = tx.send(Event::JobStarted(index));
        let (tx, cancel) = (tx.clone(), cancel.clone());
        tasks.spawn(async move {
            let _permit = permit;
            let outcome = run_job(index, &job, &tx, &cancel, false).await;
            let _ = tx.send(Event::JobFinished {
                job: index,
                outcome,
            });
        });
    }

    while tasks.join_next().await.is_some() {}
    let _ = tx.send(Event::AllDone);
}

/// Runs one job: starts ffmpeg, forwards its progress and log lines as events,
/// and kills it when `cancel` is set.
///
/// With `own_process_group`, ffmpeg does not receive the terminal's Ctrl+C. The
/// service needs that: it finishes running jobs on the first Ctrl+C instead of
/// losing them. The interactive modes rely on Ctrl+C reaching ffmpeg directly.
pub async fn run_job(
    index: usize,
    job: &Job,
    tx: &Sender<Event>,
    cancel: &AtomicBool,
    own_process_group: bool,
) -> Outcome {
    // Start clean so old segments from an earlier run don't mix in. (For
    // uploads to a bucket the service clears the old output before the job.)
    if let OutDir::Local(dir) = &job.dir {
        let _ = fs::remove_dir_all(dir);
        if let Err(e) = fs::create_dir_all(dir) {
            return Outcome::Failed(format!("cannot create {}: {e}", dir.display()));
        }
    }
    let _ = tx.send(Event::Log {
        job: index,
        line: format!("$ {}", job.command_line()),
    });

    let mut command = Command::new("ffmpeg");
    #[cfg(unix)]
    if own_process_group {
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = own_process_group;
    let mut child = match command
        .args(&job.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return Outcome::Failed(format!("could not start ffmpeg: {e}")),
    };

    // ffmpeg's normal log goes to stderr: forward every line from a separate task.
    let stderr = child.stderr.take().expect("stderr is piped");
    let log_tx = tx.clone();
    let log_task = tokio::spawn(async move {
        // ffmpeg ends some lines with \r instead of \n, so split on both.
        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::new();
        while reader
            .read_until(b'\n', &mut buf)
            .await
            .is_ok_and(|n| n > 0)
        {
            for line in String::from_utf8_lossy(&buf).split('\r') {
                let line = line.trim_end();
                if !line.is_empty() {
                    let _ = log_tx.send(Event::Log {
                        job: index,
                        line: redact(line).into_owned(),
                    });
                }
            }
            buf.clear();
        }
    });

    // -progress pipe:1 writes blocks of key=value lines, each ending with progress=...
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = BufReader::new(stdout).lines();
    let (mut seconds, mut speed) = (0.0, None);
    let mut cancelled = false;
    while let Ok(Some(line)) = lines.next_line().await {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            let _ = child.start_kill();
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "out_time_us" => {
                if let Ok(us) = value.parse::<f64>() {
                    seconds = (us / 1_000_000.0).max(0.0);
                }
            }
            "speed" => speed = value.trim().trim_end_matches('x').parse().ok(),
            "progress" => {
                let _ = tx.send(Event::Progress {
                    job: index,
                    seconds,
                    speed,
                });
            }
            _ => {}
        }
    }

    let status = child.wait().await;
    let _ = log_task.await;
    match status {
        _ if cancelled => Outcome::Cancelled,
        Ok(s) if s.success() => Outcome::Done,
        Ok(s) => Outcome::Failed(format!("ffmpeg failed ({s})")),
        Err(e) => Outcome::Failed(format!("ffmpeg did not finish: {e}")),
    }
}
