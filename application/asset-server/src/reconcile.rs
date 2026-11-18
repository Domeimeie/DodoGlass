//! The reconciler: the transcoder's output marker (`<id>/source.json` in
//! `final`) decides when a Media file is Ready (ADR 0001). It runs at
//! startup and periodically, so a finished transcode is found even when the
//! asset server was down or missed the status message.

use std::sync::Arc;

use anyhow::{Context, Result};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use serde::Deserialize;
use tracing::{info, warn};

use crate::catalogue::{Catalogue, Manifests};

#[derive(Clone)]
pub struct Reconciler {
    catalogue: Catalogue,
    final_: Arc<dyn ObjectStore>,
    /// The `final` bucket's name, the first part of the manifest keys.
    final_bucket: String,
}

/// The parts of `source.json` the asset server reads.
#[derive(Deserialize)]
struct OutputMarker {
    formats: Vec<String>,
}

impl Reconciler {
    pub fn new(catalogue: Catalogue, final_: Arc<dyn ObjectStore>, final_bucket: &str) -> Self {
        Reconciler {
            catalogue,
            final_,
            final_bucket: final_bucket.into(),
        }
    }

    /// One pass: every Processing Media file whose output marker exists
    /// becomes Ready.
    pub async fn run_once(&self) -> Result<()> {
        for id in self.catalogue.processing_ids().await? {
            match self.marker(&id).await {
                Ok(Some(marker)) => {
                    let manifests = self.manifests(&id, &marker);
                    self.catalogue.output_found(&id, manifests).await?;
                    info!(media_file = %id, "ready");
                }
                Ok(None) => {}
                // One unreadable marker must not hold up the others.
                Err(e) => warn!(media_file = %id, error = format!("{e:#}"), "output marker unreadable"),
            }
        }
        Ok(())
    }

    async fn marker(&self, id: &str) -> Result<Option<OutputMarker>> {
        let path = Path::parse(format!("{id}/source.json"))?;
        let bytes = match self.final_.get(&path).await {
            Ok(result) => result.bytes().await?,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(Some(
            serde_json::from_slice(&bytes).context("invalid source.json")?,
        ))
    }

    fn manifests(&self, id: &str, marker: &OutputMarker) -> Manifests {
        let has = |format: &str| marker.formats.iter().any(|f| f == format);
        let key = |file: &str| format!("{}/{id}/{file}", self.final_bucket);
        Manifests {
            hls: has("hls").then(|| key("hls/master.m3u8")),
            dash: has("dash").then(|| key("dash/manifest.mpd")),
        }
    }
}
