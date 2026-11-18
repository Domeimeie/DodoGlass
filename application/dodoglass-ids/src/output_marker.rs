//! The output marker `final/<id>/source.json`: written by the transcoder
//! once an output is complete, read by the asset server to decide Ready
//! (ADR 0001) and by the transcoder to skip an unchanged re-upload.

use serde::{Deserialize, Serialize};

use crate::same_key;

/// The output marker's contents. `F` is how a format is spelled: the
/// transcoder's own `Format` enum, or a plain `String` (`"hls"`, `"dash"`).
///
/// ```json
/// {"input": {"bucket": "ingest", "key": "sample.mp4"}, "etag": "…",
///  "formats": ["hls", "dash"], "completed_ms": 1793128170038, "worker": "worker-1"}
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputMarker<F> {
    pub input: SourceObject,
    /// The eTag of the input version that was transcoded.
    pub etag: String,
    pub formats: Vec<F>,
    pub completed_ms: u64,
    pub worker: String,
}

/// The object an output was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceObject {
    pub bucket: String,
    pub key: String,
}

impl<F: PartialEq> OutputMarker<F> {
    /// Whether this output was made from exactly this version of this object,
    /// in these formats, so transcoding it again would change nothing. The NFC
    /// and NFD spellings of a key are the same video (same id, ADR 0004).
    pub fn describes(&self, bucket: &str, key: &str, etag: &str, formats: &[F]) -> bool {
        self.input.bucket == bucket
            && same_key(&self.input.key, key)
            && self.etag == etag
            && self.formats == formats
    }
}
