//! Plain text output, for scripts, CI and servers without a terminal.

use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver as Receiver;

use crate::ffmpeg::{Job, Video};
use crate::runner::{Event, Outcome};

/// Prints progress until all jobs are done. Returns true if every video and job succeeded.
pub fn run(
    videos: &[Video],
    jobs: &[Job],
    threads: usize,
    warnings: &[String],
    mut rx: Receiver<Event>,
) -> bool {
    println!(
        "Transcoding {} video(s): {} job(s), up to {threads} at a time",
        videos.len(),
        jobs.len()
    );
    for w in warnings {
        eprintln!("warning: {w}");
    }
    let probe_failures = videos.iter().filter(|v| v.source.is_err()).count();
    for v in videos {
        if let Err(e) = &v.source {
            eprintln!("✘ {}: {e}", v.name);
        }
    }

    let label = |i: usize| {
        format!(
            "[{} {}]",
            videos[jobs[i].video].name,
            jobs[i].format.label()
        )
    };
    let mut started = vec![Instant::now(); jobs.len()];
    let mut last_print: Vec<Option<Instant>> = vec![None; jobs.len()];
    let (mut done, mut failed, mut cancelled) = (0, 0, 0);

    while let Some(event) = rx.blocking_recv() {
        match event {
            Event::JobStarted(i) => {
                started[i] = Instant::now();
                println!("▶ {} → {}", label(i), jobs[i].manifest.display());
            }
            Event::Progress {
                job,
                seconds,
                speed,
                ..
            } => {
                // ffmpeg reports twice a second per job; every 2 s is plenty here.
                if last_print[job].is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
                    continue;
                }
                last_print[job] = Some(Instant::now());
                let duration = videos[jobs[job].video].duration();
                let pct = if duration > 0.0 {
                    (seconds / duration * 100.0).clamp(0.0, 100.0)
                } else {
                    0.0
                };
                let speed = speed
                    .map(|s| format!("{s:.2}x"))
                    .unwrap_or_else(|| "-".into());
                println!(
                    "  {} {pct:5.1}%  {}/{}  speed {speed}",
                    label(job),
                    clock(seconds),
                    clock(duration)
                );
            }
            Event::Log { job, line } => eprintln!("{} {line}", label(job)),
            Event::JobFinished { job, outcome } => match outcome {
                Outcome::Done => {
                    done += 1;
                    println!(
                        "✔ {} done in {:.1}s → {}",
                        label(job),
                        started[job].elapsed().as_secs_f64(),
                        jobs[job].manifest.display()
                    );
                }
                Outcome::Failed(e) => {
                    failed += 1;
                    eprintln!("✘ {}: {e}", label(job));
                }
                Outcome::Cancelled => cancelled += 1,
            },
            Event::AllDone => break,
        }
    }

    println!(
        "Finished: {done} done, {failed} failed, {cancelled} cancelled, {probe_failures} unreadable"
    );
    failed == 0 && cancelled == 0 && probe_failures == 0
}

pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}
