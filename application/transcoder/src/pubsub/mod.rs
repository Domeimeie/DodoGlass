//! The messaging interface of the service: where tasks come from and where
//! status goes. `mock.rs` implements it with plain files; a Valkey (Redis)
//! implementation can be added next to it without touching the service.

pub mod mock;

use std::future::Future;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::ffmpeg::Format;

/// Channel the service takes tasks from.
pub const TASK_CHANNEL: &str = "transcode.tasks";
/// Channel the service publishes status messages to.
pub const STATUS_CHANNEL: &str = "transcode.status";

/// A transcoding task, as published by whoever wants a video transcoded.
///
/// ```json
/// {"id": "holiday-2024", "input": "/data/in/holiday.mp4", "formats": ["hls", "dash"]}
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    /// Unique task name; also the output folder name. The mock defaults it to
    /// the message's file name.
    #[serde(default)]
    pub id: String,
    /// The video file, as seen by the service (inside a container: a mounted path).
    pub input: PathBuf,
    /// Which formats to produce; defaults to the service's `--format`.
    pub formats: Option<Vec<Format>>,
}

impl Task {
    /// The id becomes a folder name, so keep it to safe characters.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            || self.id.starts_with('.')
        {
            return Err(format!(
                "task id {:?} may only contain letters, digits, '-', '_' and '.'",
                self.id
            ));
        }
        if self.formats.as_ref().is_some_and(|f| f.is_empty()) {
            return Err("formats must not be empty".into());
        }
        Ok(())
    }
}

/// What a worker announces when it registers.
#[derive(Debug, Clone, Serialize)]
pub struct WorkerInfo {
    pub id: String,
    pub pid: u32,
    /// How many tasks it runs at the same time.
    pub slots: usize,
    pub started_ms: u64,
    pub version: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusEvent {
    /// The worker took the task.
    Accepted,
    /// The message could not be read as a task.
    Rejected,
    JobStarted,
    Progress,
    JobDone,
    JobFailed,
    /// Every job of the task succeeded.
    Completed,
    /// The task did not complete (unreadable input, failed or cancelled job).
    Failed,
}

/// One status update, published to [`STATUS_CHANNEL`].
#[derive(Debug, Clone, Serialize)]
pub struct StatusMessage {
    pub time_ms: u64,
    pub worker: String,
    pub task: String,
    pub event: StatusEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl StatusMessage {
    pub fn new(worker: &str, task: &str, event: StatusEvent) -> Self {
        StatusMessage {
            time_ms: now_ms(),
            worker: worker.into(),
            task: task.into(),
            event,
            format: None,
            percent: None,
            manifest: None,
            error: None,
        }
    }
}

/// Receives tasks. One per service instance.
pub trait PubSub {
    type Publisher: Publisher + Clone + Send + Sync + 'static;

    /// Announce this worker and start listening.
    async fn register(&mut self, worker: &WorkerInfo) -> Result<()>;
    /// Wait until a task arrives. Each task is delivered to only one worker.
    async fn next_task(&mut self) -> Result<Task>;
    /// A handle for publishing status, usable from any task.
    fn publisher(&self) -> Self::Publisher;
    /// Remove this worker's registration.
    async fn unregister(&mut self) -> Result<()>;
}

/// Publishes status messages.
pub trait Publisher {
    fn publish(&self, message: &StatusMessage) -> impl Future<Output = Result<()>> + Send;
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
