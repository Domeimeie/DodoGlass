//! Turns an S3 bucket notification, as RustFS publishes it over MQTT, into
//! the objects (bucket + key) that were created.
//!
//! RustFS wraps the standard S3 event format (`Records[].data`, the same as
//! Amazon S3 and MinIO use) in an envelope of its own. Only the standard part
//! is read, so this also works with other S3 storage.

use percent_encoding::percent_decode_str;
use serde::Deserialize;

/// One created object.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub bucket: String,
    pub key: String,
    pub size: u64,
    /// Identifies this version of the object: changes whenever its content
    /// does (for multipart uploads it also depends on the part sizes).
    pub etag: Option<String>,
}

// The JSON shape. Only the fields that are needed are declared: serde skips
// the rest of the message (request headers etc.) without building it in memory.

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
    #[serde(default)]
    size: u64,
    #[serde(rename = "eTag", default)]
    etag: Option<String>,
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
            size: r.data.s3.object.size,
            etag: r
                .data
                .s3
                .object
                .etag
                .map(|e| e.trim_matches('"').to_string())
                .filter(|e| !e.is_empty()),
        })
        .collect())
}

/// S3 event keys are form-encoded: "my+movie%282%29.mp4" -> "my movie(2).mp4".
/// '+' must become a space *before* decoding %XX, otherwise an encoded plus
/// ("%2B") would first turn into '+' and then wrongly into a space.
fn decode_key(key: &str) -> String {
    percent_decode_str(&key.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_rustfs_event() {
        let payload = include_bytes!("../../tests/fixtures/rustfs-event.json");
        assert_eq!(
            objects(payload).unwrap(),
            vec![Object {
                bucket: "ingest".into(),
                key: "sample.mp4".into(),
                size: 20966549,
                etag: Some("748965504cce42abd1c302a2971f98c7-2".into()),
            }]
        );
    }

    #[test]
    fn reads_several_records() {
        let payload = br#"{"Records":[
            {"data":{"s3":{"bucket":{"name":"ingest"},"object":{"key":"a.mp4","size":1}}}},
            {"data":{"s3":{"bucket":{"name":"ingest"},"object":{"key":"b.mkv","size":2}}}}]}"#;
        let keys: Vec<_> = objects(payload)
            .unwrap()
            .into_iter()
            .map(|o| o.key)
            .collect();
        assert_eq!(keys, ["a.mp4", "b.mkv"]);
    }

    #[test]
    fn decodes_keys() {
        assert_eq!(decode_key("sample.mp4"), "sample.mp4");
        assert_eq!(decode_key("my+movie.mp4"), "my movie.mp4");
        assert_eq!(decode_key("a%2Bb%20%28final%29.mkv"), "a+b (final).mkv");
        assert_eq!(decode_key("caf%C3%A9.mov"), "café.mov");
    }

    #[test]
    fn rejects_garbage() {
        assert!(objects(b"hello").is_err());
        assert!(objects(br#"{"something":"else"}"#).is_err());
    }
}
