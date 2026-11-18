//! The buckets, through `object_store`: S3 (RustFS) in production, in memory
//! in tests. Keys always go through `Path::parse`, never `Path::from`, which
//! would percent-encode characters such as `[` a second time.

use std::sync::Arc;

use anyhow::{Context, Result};
use object_store::ObjectStore;
use object_store::aws::AmazonS3Builder;

/// One store per bucket.
#[derive(Clone)]
pub struct Buckets {
    /// Uploads (`movies/...`, `shows/...`).
    pub ingest: Arc<dyn ObjectStore>,
    /// The transcoder's outputs: `<task id>/...` with `<task id>/source.json`.
    pub final_: Arc<dyn ObjectStore>,
    /// Images the asset server stores.
    pub assets: Arc<dyn ObjectStore>,
}

pub struct S3Settings {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

impl Buckets {
    pub fn s3(s3: &S3Settings, ingest: &str, final_: &str, assets: &str) -> Result<Buckets> {
        let bucket = |name: &str| -> Result<Arc<dyn ObjectStore>> {
            let store = AmazonS3Builder::new()
                .with_endpoint(&s3.endpoint)
                .with_allow_http(s3.endpoint.starts_with("http://"))
                // Path-style requests: RustFS has no per-bucket host names.
                .with_virtual_hosted_style_request(false)
                .with_region(&s3.region)
                .with_access_key_id(&s3.access_key)
                .with_secret_access_key(&s3.secret_key)
                .with_bucket_name(name)
                .build()
                .with_context(|| format!("bucket {name}"))?;
            Ok(Arc::new(store))
        };
        Ok(Buckets {
            ingest: bucket(ingest)?,
            final_: bucket(final_)?,
            assets: bucket(assets)?,
        })
    }

    /// Lists each bucket once, so that a wrong endpoint, credentials or bucket
    /// name fail at startup.
    pub async fn check(&self) -> Result<()> {
        for (name, store) in [
            ("ingest", &self.ingest),
            ("final", &self.final_),
            ("assets", &self.assets),
        ] {
            store
                .list_with_delimiter(None)
                .await
                .with_context(|| format!("cannot list the {name} bucket"))?;
        }
        Ok(())
    }
}
