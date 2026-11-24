//! The reconciler: the transcoder's output marker (`<id>/source.json` in
//! `final`) decides when a Media file is Ready (ADR 0001). The marker
//! listener checks one Media file as soon as RustFS announces its marker
//! (ADR 0006); a full pass runs at startup and periodically, the safety net
//! for events missed while the asset server was down or lost on the way.
//!
//! One pass checks every Media file against its marker:
//! - marker there: Ready (also from Missing), with the formats it lists;
//! - marker gone from a Ready one: Missing;
//!
//! and places Media files from markers whose folder it does not know.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use tracing::{info, warn};

use crate::catalogue::{Catalogue, Manifests, MediaFileState, TrackedMediaFile};

#[derive(Clone)]
pub struct Reconciler {
    catalogue: Catalogue,
    final_: Arc<dyn ObjectStore>,
    /// The `final` bucket's name, the first part of the manifest keys.
    final_bucket: String,
}

/// `source.json`, as the transcoder writes it (schema shared through
/// `dodoglass-ids`). The asset server reads its formats, and its input key
/// for folders it does not know.
type OutputMarker = dodoglass_ids::OutputMarker<String>;

/// What became of a marker the marker listener was told about.
#[derive(Debug)]
pub enum MarkerOutcome {
    /// The catalogue agrees with the marker (also when it already did).
    Stored,
    /// The marker is not a valid `source.json`: reading it again will not help.
    Unreadable(String),
}

impl Reconciler {
    pub fn new(catalogue: Catalogue, final_: Arc<dyn ObjectStore>, final_bucket: &str) -> Self {
        Reconciler {
            catalogue,
            final_,
            final_bucket: final_bucket.into(),
        }
    }

    /// The `final` bucket's name, as bucket events name it.
    pub fn final_bucket(&self) -> &str {
        &self.final_bucket
    }

    /// One pass over every Media file and every folder in `final`.
    pub async fn run_once(&self) -> Result<()> {
        let files = self.catalogue.tracked_media_files().await?;
        let known: HashSet<&str> = files.iter().map(|f| f.id.as_str()).collect();
        for file in &files {
            // One unreadable marker must not hold up the others.
            if let Err(e) = self.check(file).await {
                warn!(media_file = %file.id, error = format!("{e:#}"), "could not reconcile");
            }
        }
        let folders = self.final_.list_with_delimiter(None).await?.common_prefixes;
        for folder in folders {
            let id = folder.as_ref();
            if known.contains(id) {
                continue;
            }
            if let Err(e) = self.check_unknown(id).await {
                warn!(folder = %id, error = format!("{e:#}"), "could not reconcile");
            }
        }
        Ok(())
    }

    /// Checks Media file `id` alone; an unknown `id` is placed from its
    /// marker, if there is one.
    async fn run_for(&self, id: &str) -> Result<()> {
        match self.catalogue.tracked_media_file(id).await? {
            Some(file) => self.check(&file).await,
            None => self.check_unknown(id).await,
        }
    }

    /// RustFS announced a new output marker for Media file `id`: it is
    /// checked alone, as one pass would check it. The marker is read from
    /// `final` rather than trusted from the event, so a marker already
    /// deleted again (a re-upload started) makes it Missing, not Ready.
    pub async fn marker_seen(&self, id: &str) -> Result<MarkerOutcome> {
        match self.run_for(id).await {
            Ok(()) => Ok(MarkerOutcome::Stored),
            Err(e) if e.downcast_ref::<serde_json::Error>().is_some() => {
                Ok(MarkerOutcome::Unreadable(format!("{e:#}")))
            }
            Err(e) => Err(e),
        }
    }

    async fn check(&self, file: &TrackedMediaFile) -> Result<()> {
        let id = &file.id;
        match (self.marker(id).await?, file.state) {
            (Some(marker), state) => {
                let manifests = self.manifests(id, &marker);
                if state == MediaFileState::Ready && manifests == file.manifests {
                    return Ok(());
                }
                self.catalogue.output_found(id, manifests).await?;
                info!(media_file = %id, "ready");
            }
            (None, MediaFileState::Ready) => {
                self.catalogue.output_lost(id).await?;
                warn!(media_file = %id, "output marker gone from final: missing");
            }
            (None, MediaFileState::Missing) => {}
        }
        Ok(())
    }

    /// A folder in `final` with no Media file: placed from the input key its
    /// marker records, if that key gives this folder's id.
    async fn check_unknown(&self, id: &str) -> Result<()> {
        let Some(marker) = self.marker(id).await? else {
            return Ok(());
        };
        let key = marker.input.key.as_str();
        if dodoglass_ids::task_id_from_key(key) != id {
            warn!(folder = %id, %key, "source.json input key does not match its folder");
            return Ok(());
        }
        let manifests = self.manifests(id, &marker);
        self.catalogue.output_seen(key, manifests).await?;
        info!(media_file = %id, %key, "placed from an output marker: ready");
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
