//! The messaging interface of the service: where tasks come from and where
//! status goes. `mock.rs` implements it with plain files, `mqtt.rs` with an
//! MQTT broker that receives bucket events from RustFS.

pub mod mock;
pub mod mqtt;
pub mod s3_event;

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
/// {"id": "holiday-2024", "input": {"bucket": "ingest", "key": "holiday.mp4"}}
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    /// Unique task name; also the output folder name. The mock defaults it to
    /// the message's file name.
    #[serde(default)]
    pub id: String,
    pub input: Input,
    /// Which formats to produce; defaults to the service's `--format`.
    pub formats: Option<Vec<Format>>,
    /// Confirms the message to the broker once the task is finished (or given
    /// up), when the last clone of the task is dropped. Not part of the JSON.
    #[serde(skip)]
    #[allow(dead_code, reason = "never read: it does its work when dropped")]
    pub ack: Ack,
}

/// Where the video is.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Input {
    /// A file, as seen by the service (inside a container: a mounted path).
    File(PathBuf),
    /// An object in S3 storage. It is never downloaded: ffmpeg reads it over
    /// HTTP, only the parts it needs.
    Object {
        bucket: String,
        key: String,
        /// The version to read (from the bucket event). ffmpeg reads with
        /// `If-Match`, so a file replaced mid-transcode fails cleanly instead
        /// of mixing two versions. Also used to skip unchanged re-uploads.
        #[serde(default)]
        etag: Option<String>,
    },
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input::File(path) => write!(f, "{}", path.display()),
            Input::Object { bucket, key, .. } => write!(f, "s3://{bucket}/{key}"),
        }
    }
}

/// The video file types the transcoder accepts.
pub const VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "mov", "mkv"];

/// Whether a file or object name ends in one of [`VIDEO_EXTENSIONS`] (any case).
pub fn is_video_name(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| VIDEO_EXTENSIONS.iter().any(|v| ext.eq_ignore_ascii_case(v)))
}

/// Runs a function once, when the last clone of this handle is dropped. The
/// default handle does nothing (the mock needs no acknowledgement).
///
/// Dropping is what makes it reliable: however a task ends (done, failed,
/// early return), its message is acknowledged exactly once.
#[derive(Clone, Default)]
pub struct Ack(Option<Arc<AckOnDrop>>);

struct AckOnDrop(Mutex<Option<Box<dyn FnOnce() + Send>>>);

impl Ack {
    pub fn new(on_last_drop: impl FnOnce() + Send + 'static) -> Self {
        Ack(Some(Arc::new(AckOnDrop(Mutex::new(Some(Box::new(
            on_last_drop,
        )))))))
    }
}

impl Drop for AckOnDrop {
    fn drop(&mut self) {
        if let Some(f) = self.0.get_mut().ok().and_then(Option::take) {
            f();
        }
    }
}

impl fmt::Debug for Ack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "Ack" } else { "Ack(none)" })
    }
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
        let name = match &self.input {
            Input::File(path) => path.to_string_lossy().into_owned(),
            Input::Object { key, .. } => key.clone(),
        };
        if !is_video_name(&name) {
            return Err(format!(
                "{} is not a video file (accepted: .{})",
                self.input,
                VIDEO_EXTENSIONS.join(", .")
            ));
        }
        Ok(())
    }
}

/// A task id (also the output folder name) from an object key:
/// "shows/Episode 1.mkv" -> "shows_Episode_1". Characters other than letters,
/// digits, '-', '_' and '.' become '_'.
pub fn task_id_from_key(key: &str) -> String {
    let stem = match key.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => key,
    };
    let id: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let id = id.trim_start_matches('.');
    if id.is_empty() {
        "video".into()
    } else {
        id.into()
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
    /// The same version of this video was already transcoded; nothing to do.
    Skipped,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn task_ids_from_keys() {
        assert_eq!(task_id_from_key("sample.mp4"), "sample");
        assert_eq!(task_id_from_key("shows/Episode 1.mkv"), "shows_Episode_1");
        assert_eq!(task_id_from_key("café.mov"), "caf_");
        assert_eq!(task_id_from_key(".hidden.mp4"), "hidden");
        assert_eq!(task_id_from_key(".mp4"), "mp4");
        assert_eq!(task_id_from_key("..."), "video");
    }

    #[test]
    fn video_names() {
        assert!(is_video_name("a.mp4"));
        assert!(is_video_name("dir/A.MOV"));
        assert!(is_video_name("x.MkV"));
        assert!(!is_video_name("notes.txt"));
        assert!(!is_video_name("mp4"));
    }

    #[test]
    fn input_is_a_path_or_an_object() {
        let file: Task = serde_json::from_str(r#"{"id":"a","input":"/in/a.mp4"}"#).unwrap();
        assert_eq!(file.input, Input::File("/in/a.mp4".into()));
        let object: Task =
            serde_json::from_str(r#"{"id":"a","input":{"bucket":"ingest","key":"a.mkv"}}"#)
                .unwrap();
        assert_eq!(object.input.to_string(), "s3://ingest/a.mkv");
        assert!(object.validate().is_ok());
    }

    #[test]
    fn rejects_non_videos() {
        let task: Task = serde_json::from_str(r#"{"id":"a","input":"/in/a.txt"}"#).unwrap();
        assert!(task.validate().unwrap_err().contains("not a video"));
    }

    #[test]
    fn ack_runs_once_when_the_last_clone_is_dropped() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let ack = Ack::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });
        let (a, b) = (ack.clone(), ack.clone());
        drop(ack);
        drop(a);
        assert_eq!(count.load(Ordering::Relaxed), 0);
        drop(b);
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
