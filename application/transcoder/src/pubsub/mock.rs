//! A PubSub stand-in that uses plain files, for development without a broker.
//!
//! ```text
//! <root>/
//! ├── transcode.tasks/          ← drop a <name>.json here to publish a task
//! │   ├── claimed/              ← tasks a worker took (renamed: <name>.json.<worker>)
//! │   └── rejected/             ← messages that were not valid tasks
//! ├── transcode.status.jsonl    ← status messages, one JSON object per line
//! └── workers/<worker>.json     ← registered workers
//! ```
//!
//! Claiming a task is a rename, which the file system does atomically: if two
//! workers try to take the same task, only one rename succeeds. That gives the
//! same "each task goes to exactly one worker" behaviour as a real queue.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::warn;

use super::{
    PubSub, Publisher, STATUS_CHANNEL, StatusEvent, StatusMessage, TASK_CHANNEL, Task, WorkerInfo,
};

/// How often the inbox is checked for new task files.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct MockPubSub {
    root: PathBuf,
    worker: String,
}

#[derive(Clone)]
pub struct MockPublisher {
    status_file: PathBuf,
}

impl MockPubSub {
    pub fn new(root: PathBuf, worker: &str) -> Self {
        MockPubSub {
            root,
            worker: worker.into(),
        }
    }

    fn inbox(&self) -> PathBuf {
        self.root.join(TASK_CHANNEL)
    }

    fn worker_file(&self) -> PathBuf {
        self.root
            .join("workers")
            .join(format!("{}.json", self.worker))
    }

    /// Takes the oldest task file (by name) that no other worker took first.
    fn try_claim(&self) -> Result<Option<Task>> {
        let inbox = self.inbox();
        let mut names: Vec<String> = fs::read_dir(&inbox)
            .with_context(|| format!("cannot read {}", inbox.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort();

        for name in names {
            let claimed = inbox
                .join("claimed")
                .join(format!("{name}.{}", self.worker));
            // Another worker may have been faster: then the rename fails, try the next.
            if fs::rename(inbox.join(&name), &claimed).is_err() {
                continue;
            }
            let fallback_id = name.trim_end_matches(".json").to_string();
            match read_task(&claimed, &fallback_id) {
                Ok(task) => return Ok(Some(task)),
                Err(reason) => {
                    warn!(message_file = %name, %reason, "rejected task message");
                    let _ = fs::rename(&claimed, inbox.join("rejected").join(&name));
                    let mut status =
                        StatusMessage::new(&self.worker, &fallback_id, StatusEvent::Rejected);
                    status.error = Some(reason);
                    let _ = append_line(&self.publisher().status_file, &status);
                }
            }
        }
        Ok(None)
    }
}

fn read_task(path: &Path, fallback_id: &str) -> Result<Task, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut task: Task =
        serde_json::from_str(&text).map_err(|e| format!("invalid task JSON: {e}"))?;
    if task.id.is_empty() {
        task.id = fallback_id.to_string();
    }
    task.validate()?;
    Ok(task)
}

impl PubSub for MockPubSub {
    type Publisher = MockPublisher;

    async fn register(&mut self, worker: &WorkerInfo) -> Result<()> {
        for dir in [
            self.inbox().join("claimed"),
            self.inbox().join("rejected"),
            self.root.join("workers"),
        ] {
            fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        fs::write(self.worker_file(), serde_json::to_string_pretty(worker)?)
            .context("cannot write worker registration")?;
        Ok(())
    }

    async fn next_task(&mut self) -> Result<Task> {
        loop {
            if let Some(task) = self.try_claim()? {
                return Ok(task);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    fn publisher(&self) -> MockPublisher {
        MockPublisher {
            status_file: self.root.join(format!("{STATUS_CHANNEL}.jsonl")),
        }
    }

    async fn unregister(&mut self) -> Result<()> {
        let _ = fs::remove_file(self.worker_file());
        Ok(())
    }
}

impl Publisher for MockPublisher {
    async fn publish(&self, message: &StatusMessage) -> Result<()> {
        append_line(&self.status_file, message)
    }
}

/// Appends one JSON line. A single small append is atomic, so concurrent
/// publishers don't interleave their lines.
fn append_line(path: &Path, message: &StatusMessage) -> Result<()> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .with_context(|| format!("cannot write {}", path.display()))
}
