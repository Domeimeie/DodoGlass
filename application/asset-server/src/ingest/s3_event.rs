//! Reads the created objects out of an S3 bucket notification.
//!
//! RustFS wraps the standard S3 event format (`Records[].data`) in an
//! envelope of its own. Only the standard part is read, as the transcoder
//! does, so this also works with other S3 storage.

use percent_encoding::percent_decode_str;
use serde::Deserialize;

/// One created object.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub bucket: String,
    /// URL-decoded.
    pub key: String,
}

#[derive(Deserialize)]
struct Notification {
    #[serde(rename = "Records")]
    records: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    data: EventData,
}

#[derive(Deserialize)]
struct EventData {
    s3: S3Entity,
}

#[derive(Deserialize)]
struct S3Entity {
    bucket: BucketEntity,
    object: ObjectEntity,
}

#[derive(Deserialize)]
struct BucketEntity {
    name: String,
}

#[derive(Deserialize)]
struct ObjectEntity {
    key: String,
}

/// All objects in one notification message (a message can hold several).
pub fn objects(payload: &[u8]) -> Result<Vec<Object>, String> {
    let notification: Notification =
        serde_json::from_slice(payload).map_err(|e| format!("not an S3 event: {e}"))?;
    Ok(notification
        .records
        .into_iter()
        .map(|r| Object {
            bucket: r.data.s3.bucket.name,
            key: decode_key(&r.data.s3.object.key),
        })
        .collect())
}

/// S3 event keys are form-encoded: "my+movie%282%29.mp4" -> "my movie(2).mp4".
/// '+' becomes a space *before* %XX is decoded, so "%2B" stays a '+'.
fn decode_key(key: &str) -> String {
    percent_decode_str(&key.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}
