//! Access to S3-compatible object storage (RustFS): presigned read URLs for
//! ffmpeg, uploads of finished files, and removing old output.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use futures_util::StreamExt;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path;
use object_store::signer::{Method, Signer};
use object_store::{Attribute, Attributes, ObjectStore, ObjectStoreExt, PutOptions, PutPayload};

/// How long a presigned read URL stays valid. ffmpeg keeps using the URL for
/// the whole transcode (every read is a new range request), so this must be
/// longer than the longest transcode.
pub const READ_URL_VALIDITY: Duration = Duration::from_secs(12 * 3600);

#[derive(Debug, Clone)]
pub struct S3Settings {
    /// e.g. `http://localhost:9000`
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

/// One client per bucket (that is how object_store addresses buckets),
/// created on first use and shared afterwards.
pub struct Storage {
    settings: S3Settings,
    buckets: Mutex<HashMap<String, Arc<AmazonS3>>>,
    /// For the one request object_store cannot make: a conditional DELETE.
    http: reqwest::Client,
}

/// What [`Storage::delete_if_unchanged`] did.
#[derive(Debug, PartialEq)]
pub enum Removal {
    Deleted,
    /// The object is a different version now (a newer upload): it was kept.
    Changed,
    /// There was nothing to delete.
    Gone,
}

impl Storage {
    pub fn new(settings: S3Settings) -> Self {
        Storage {
            settings,
            buckets: Mutex::new(HashMap::new()),
            http: reqwest::Client::new(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.settings.endpoint
    }

    fn bucket(&self, name: &str) -> Result<Arc<AmazonS3>> {
        let mut buckets = self.buckets.lock().expect("storage lock poisoned");
        if let Some(store) = buckets.get(name) {
            return Ok(store.clone());
        }
        let s = &self.settings;
        let store = AmazonS3Builder::new()
            .with_endpoint(&s.endpoint)
            .with_allow_http(s.endpoint.starts_with("http://"))
            // http://host:9000/<bucket>/<key>: RustFS has no per-bucket hostnames.
            .with_virtual_hosted_style_request(false)
            .with_region(&s.region)
            .with_access_key_id(&s.access_key)
            .with_secret_access_key(&s.secret_key)
            .with_bucket_name(name)
            .build()
            .with_context(|| format!("invalid S3 settings for bucket {name}"))?;
        let store = Arc::new(store);
        buckets.insert(name.to_string(), store.clone());
        Ok(store)
    }

    /// Fails if the bucket does not exist or the credentials are wrong; used
    /// at startup so a configuration error shows immediately.
    pub async fn check_bucket(&self, name: &str) -> Result<()> {
        self.bucket(name)?
            .list_with_delimiter(None)
            .await
            .with_context(|| format!("cannot access bucket {name} at {}", self.endpoint()))?;
        Ok(())
    }

    /// A URL anyone holding it can GET the object with, until it expires.
    /// ffmpeg reads the video through it directly from the storage, with
    /// HTTP range requests: only the parts it needs, when it needs them.
    pub async fn read_url(&self, bucket: &str, key: &str) -> Result<String> {
        let url = self
            .bucket(bucket)?
            .signed_url(Method::GET, &object_path(key)?, READ_URL_VALIDITY)
            .await
            .with_context(|| format!("cannot sign a read URL for s3://{bucket}/{key}"))?;
        Ok(url.to_string())
    }

    /// Uploads one (small) file in a single request.
    pub async fn put(&self, bucket: &str, key: &str, data: Bytes) -> Result<()> {
        let mut attributes = Attributes::new();
        attributes.insert(Attribute::ContentType, content_type(key).into());
        let options = PutOptions {
            attributes,
            ..Default::default()
        };
        self.bucket(bucket)?
            .put_opts(&object_path(key)?, PutPayload::from(data), options)
            .await
            .with_context(|| format!("upload of s3://{bucket}/{key} failed"))?;
        Ok(())
    }

    /// Reads a small object completely; `None` if it does not exist.
    pub async fn get_small(&self, bucket: &str, key: &str) -> Result<Option<Bytes>> {
        match self.bucket(bucket)?.get(&object_path(key)?).await {
            Ok(result) => {
                Ok(Some(result.bytes().await.with_context(|| {
                    format!("cannot read s3://{bucket}/{key}")
                })?))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read s3://{bucket}/{key}")),
        }
    }

    /// Deleting something that does not exist counts as success.
    pub async fn delete(&self, bucket: &str, key: &str) -> Result<()> {
        match self.bucket(bucket)?.delete(&object_path(key)?).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e).with_context(|| format!("cannot delete s3://{bucket}/{key}")),
        }
    }

    /// Deletes the object only if it is still the version `etag`
    /// (`DELETE` with `If-Match`). Deleting by name alone could remove a newer
    /// upload with the same name that arrived in the meantime, before its
    /// own task ran. object_store has no conditional delete, so this signs a
    /// DELETE URL with it and sends the request with the header added.
    pub async fn delete_if_unchanged(
        &self,
        bucket: &str,
        key: &str,
        etag: &str,
    ) -> Result<Removal> {
        let url = self
            .bucket(bucket)?
            .signed_url(Method::DELETE, &object_path(key)?, Duration::from_secs(60))
            .await
            .with_context(|| format!("cannot sign a delete for s3://{bucket}/{key}"))?;
        let response = self
            .http
            .delete(url.as_str())
            .header("If-Match", format!("\"{}\"", etag.trim_matches('"')))
            .send()
            .await
            .with_context(|| format!("cannot delete s3://{bucket}/{key}"))?;
        match response.status().as_u16() {
            200..=299 => Ok(Removal::Deleted),
            // A condition on an object that no longer exists fails too (412),
            // so look whether it is a newer version or nothing at all.
            412 => match self.bucket(bucket)?.head(&object_path(key)?).await {
                Ok(_) => Ok(Removal::Changed),
                Err(object_store::Error::NotFound { .. }) => Ok(Removal::Gone),
                Err(e) => Err(e).with_context(|| format!("cannot check s3://{bucket}/{key}")),
            },
            404 => Ok(Removal::Gone),
            status => bail!("cannot delete s3://{bucket}/{key}: HTTP {status}"),
        }
    }

    /// Deletes every object below `prefix/`, so an earlier run's segments
    /// don't mix with the new ones. Returns how many were deleted.
    pub async fn delete_prefix(&self, bucket: &str, prefix: &str) -> Result<usize> {
        let store = self.bucket(bucket)?;
        let prefix = object_path(prefix)?;
        let locations = store
            .list(Some(&prefix))
            .map(|meta| meta.map(|m| m.location))
            .boxed();
        let mut deleted = 0;
        let mut results = store.delete_stream(locations);
        while let Some(result) = results.next().await {
            result.with_context(|| format!("cannot clean up s3://{bucket}/{prefix}"))?;
            deleted += 1;
        }
        Ok(deleted)
    }
}

/// The key as object_store's `Path`, unchanged. Not `Path::from`: that
/// percent-encodes characters object_store considers unsafe (`[`, `]`, `{`,
/// `#`, `~`, …) *into the path*, and the request URL is then encoded again, so
/// `[a].mkv` would be asked for as `%5Ba%5D.mkv` and not be found.
/// `Path::parse` keeps the key as it is; the URL is encoded exactly once.
fn object_path(key: &str) -> Result<Path> {
    Path::parse(key).with_context(|| format!("unsupported object key {key:?}"))
}

/// So that players and browsers get the right type when they fetch the files.
pub fn content_type(key: &str) -> &'static str {
    match key.rsplit_once('.').map(|(_, ext)| ext) {
        Some("m3u8") => "application/vnd.apple.mpegurl",
        Some("ts") => "video/mp2t",
        Some("mpd") => "application/dash+xml",
        Some("m4s") => "video/iso.segment",
        Some("mp4") => "video/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types() {
        assert_eq!(content_type("a/hls/720p/seg_001.ts"), "video/mp2t");
        assert_eq!(
            content_type("a/hls/master.m3u8"),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(content_type("a/dash/manifest.mpd"), "application/dash+xml");
        assert_eq!(
            content_type("a/dash/chunk-stream0-00001.m4s"),
            "video/iso.segment"
        );
        assert_eq!(content_type("README"), "application/octet-stream");
    }

    /// Against a real RustFS (a throwaway one!), with buckets `ingest` and `final`:
    /// TRANSCODER_TEST_S3=http://localhost:19000 cargo test -- --ignored
    #[tokio::test]
    #[ignore = "needs a running RustFS in TRANSCODER_TEST_S3"]
    async fn delete_prefix_only_deletes_below_the_prefix() {
        let endpoint = std::env::var("TRANSCODER_TEST_S3").expect("TRANSCODER_TEST_S3 not set");
        let storage = Storage::new(S3Settings {
            endpoint,
            access_key: "rustfsadmin".into(),
            secret_key: "rustfsadmin".into(),
            region: "us-east-1".into(),
        });
        let data = Bytes::from_static(b"x");
        storage
            .put("ingest", "sample.mp4", data.clone())
            .await
            .unwrap();
        storage
            .put("final", "sample/dash/a.m4s", data.clone())
            .await
            .unwrap();
        storage
            .put("final", "sample/dash/b.m4s", data.clone())
            .await
            .unwrap();
        storage
            .put("final", "sample/hls/master.m3u8", data)
            .await
            .unwrap();

        let deleted = storage.delete_prefix("final", "sample/dash").await.unwrap();
        assert_eq!(deleted, 2);

        let keys = |bucket: &'static str| {
            let store = storage.bucket(bucket).unwrap();
            async move {
                store
                    .list(None)
                    .map(|m| m.unwrap().location.to_string())
                    .collect::<Vec<_>>()
                    .await
            }
        };
        assert_eq!(keys("final").await, ["sample/hls/master.m3u8"]);
        // Other tests run in parallel on the same buckets, so only check that
        // the input survived (not that it is alone).
        assert!(
            keys("ingest").await.contains(&"sample.mp4".to_string()),
            "the input was deleted too"
        );
    }

    /// Same setup as above: TRANSCODER_TEST_S3=http://localhost:19000 cargo test -- --ignored
    #[tokio::test]
    #[ignore = "needs a running RustFS in TRANSCODER_TEST_S3"]
    async fn conditional_delete_keeps_a_newer_version() {
        let endpoint = std::env::var("TRANSCODER_TEST_S3").expect("TRANSCODER_TEST_S3 not set");
        let storage = Storage::new(S3Settings {
            endpoint,
            access_key: "rustfsadmin".into(),
            secret_key: "rustfsadmin".into(),
            region: "us-east-1".into(),
        });
        let store = storage.bucket("ingest").unwrap();
        let etag_of = |key: &'static str| {
            let store = store.clone();
            async move {
                store
                    .head(&object_path(key).unwrap())
                    .await
                    .unwrap()
                    .e_tag
                    .unwrap()
            }
        };

        storage
            .put("ingest", "v.mp4", Bytes::from_static(b"version 1"))
            .await
            .unwrap();
        let v1 = etag_of("v.mp4").await;
        storage
            .put("ingest", "v.mp4", Bytes::from_static(b"version 2"))
            .await
            .unwrap();
        let v2 = etag_of("v.mp4").await;
        assert_ne!(v1, v2);

        // Task for version 1 finishes after version 2 arrived: must keep version 2.
        let removal = storage
            .delete_if_unchanged("ingest", "v.mp4", &v1)
            .await
            .unwrap();
        assert_eq!(removal, Removal::Changed);
        assert_eq!(etag_of("v.mp4").await, v2);

        assert_eq!(
            storage
                .delete_if_unchanged("ingest", "v.mp4", &v2)
                .await
                .unwrap(),
            Removal::Deleted
        );
        assert_eq!(
            storage
                .delete_if_unchanged("ingest", "v.mp4", &v2)
                .await
                .unwrap(),
            Removal::Gone
        );
    }

    #[tokio::test]
    async fn presigned_url_points_at_the_object_without_exposing_the_secret() {
        let storage = Storage::new(S3Settings {
            endpoint: "http://localhost:9000".into(),
            access_key: "AKID".into(),
            secret_key: "very-secret".into(),
            region: "us-east-1".into(),
        });
        let url = storage.read_url("ingest", "my movie.mp4").await.unwrap();
        assert!(
            url.starts_with("http://localhost:9000/ingest/my%20movie.mp4?"),
            "{url}"
        );
        assert!(url.contains("X-Amz-Signature="));
        assert!(!url.contains("very-secret"));
    }

    /// Against a real RustFS: a key with brackets, spaces and other special
    /// characters can be uploaded, read through its presigned URL (the
    /// signature must match what RustFS computes) and deleted conditionally.
    /// TRANSCODER_TEST_S3=http://localhost:19000 cargo test -- --ignored
    #[tokio::test]
    #[ignore = "needs a running RustFS in TRANSCODER_TEST_S3"]
    async fn special_keys_work_against_rustfs() {
        let endpoint = std::env::var("TRANSCODER_TEST_S3").expect("TRANSCODER_TEST_S3 not set");
        let storage = Storage::new(S3Settings {
            endpoint,
            access_key: "rustfsadmin".into(),
            secret_key: "rustfsadmin".into(),
            region: "us-east-1".into(),
        });
        for key in [
            "[Judas] Kusuriya - S03E06.mkv",
            "a{b}#c~d.mp4",
            "caf\u{e9} (1).mov",
        ] {
            let data = Bytes::from(format!("content of {key}"));
            storage.put("ingest", key, data.clone()).await.unwrap();
            let url = storage.read_url("ingest", key).await.unwrap();
            let response = reqwest::get(&url).await.unwrap();
            assert_eq!(response.status(), 200, "{key}: {url}");
            assert_eq!(response.bytes().await.unwrap(), data, "{key}");
            let etag = storage
                .bucket("ingest")
                .unwrap()
                .head(&object_path(key).unwrap())
                .await
                .unwrap()
                .e_tag
                .unwrap();
            let removal = storage
                .delete_if_unchanged("ingest", key, &etag)
                .await
                .unwrap();
            assert_eq!(removal, Removal::Deleted, "{key}");
        }
    }

    /// The bug: "[Judas] Kusuriya - S03E06.mkv" was requested as %255BJudas%255D… (404).
    #[tokio::test]
    async fn special_characters_in_keys_are_encoded_once() {
        let storage = Storage::new(S3Settings {
            endpoint: "http://localhost:9000".into(),
            access_key: "AKID".into(),
            secret_key: "secret".into(),
            region: "us-east-1".into(),
        });
        let url = storage
            .read_url("ingest", "[Judas] Kusuriya - S03E06.mkv")
            .await
            .unwrap();
        assert!(
            url.starts_with(
                "http://localhost:9000/ingest/%5BJudas%5D%20Kusuriya%20-%20S03E06.mkv?"
            ),
            "{url}"
        );
        for key in ["a{b}.mp4", "c#d.mov", "e~f.mkv", "shows/S01/ep 1.mp4"] {
            let url = storage.read_url("ingest", key).await.unwrap();
            assert!(!url.contains("%25"), "{key} encoded twice: {url}");
        }
        assert!(storage.read_url("ingest", "a//b.mp4").await.is_err());
    }
}
