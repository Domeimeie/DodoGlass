//! Media file states. A Media file is Processing from its upload until the
//! transcoder's output marker is found, then Ready (ADR 0001). It is Failed
//! when its transcode did not complete, and Missing when the output of a
//! Ready one has disappeared from `final`.

use anyhow::Result;
use diesel::prelude::*;
use diesel::upsert::excluded;
use serde::Serialize;

use super::Catalogue;
use super::schema::media_file;

pub(super) const PROCESSING: &str = "processing";
pub(super) const READY: &str = "ready";
pub(super) const FAILED: &str = "failed";
pub(super) const MISSING: &str = "missing";

/// How long a Media file may stay Processing, with its input still in
/// `ingest`, before it counts as Failed.
pub const PROCESSING_TIMEOUT_MS: i64 = 24 * 60 * 60 * 1000;

/// The error of a Media file that hit [`PROCESSING_TIMEOUT_MS`].
pub const TIMED_OUT: &str = "no output after 24 h while the input is still in ingest";

/// A Media file's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaFileState {
    Processing,
    Ready,
    Failed,
    Missing,
}

impl MediaFileState {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            PROCESSING => MediaFileState::Processing,
            READY => MediaFileState::Ready,
            FAILED => MediaFileState::Failed,
            MISSING => MediaFileState::Missing,
            other => anyhow::bail!("unknown Media file state {other:?}"),
        })
    }
}

/// A Media file the reconciler checks against `final`.
#[derive(Debug, Clone)]
pub struct TrackedMediaFile {
    pub id: String,
    /// The upload key in `ingest`.
    pub source_key: String,
    pub state: MediaFileState,
    /// True when it has been Processing for longer than
    /// [`PROCESSING_TIMEOUT_MS`].
    pub overdue: bool,
}

/// The state of one Media file, for the admin API.
#[derive(Debug, Clone, Serialize)]
pub struct MediaFileStatus {
    pub id: String,
    pub state: MediaFileState,
    pub error: Option<String>,
}

/// The manifest keys of a Ready Media file (`<final bucket>/<id>/...`), for
/// the formats it was transcoded to.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Manifests {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hls: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dash: Option<String>,
}

/// Creates the Media file `id` as Processing, or makes an existing one
/// Processing again (a new version of the same upload).
pub(super) fn processing(
    conn: &mut SqliteConnection,
    id: &str,
    key: &str,
    movie: Option<i32>,
    label: &str,
    now: i64,
) -> Result<()> {
    diesel::insert_into(media_file::table)
        .values((
            media_file::id.eq(id),
            media_file::source_key.eq(key),
            media_file::movie_id.eq(movie),
            media_file::label.eq(label),
            media_file::state.eq(PROCESSING),
            media_file::state_since_ms.eq(now),
            media_file::created_ms.eq(now),
        ))
        .on_conflict(media_file::id)
        .do_update()
        .set((
            media_file::state.eq(excluded(media_file::state)),
            media_file::state_since_ms.eq(excluded(media_file::state_since_ms)),
            media_file::hls_manifest.eq(None::<String>),
            media_file::dash_manifest.eq(None::<String>),
            media_file::error.eq(None::<String>),
        ))
        .execute(conn)?;
    Ok(())
}

impl Catalogue {
    /// Every Media file, with what the reconciler needs to check it.
    pub async fn tracked_media_files(&self) -> Result<Vec<TrackedMediaFile>> {
        self.tracked(None).await
    }

    /// Media file `id`, with what the reconciler needs to check it.
    pub async fn tracked_media_file(&self, id: &str) -> Result<Option<TrackedMediaFile>> {
        Ok(self.tracked(Some(id.to_string())).await?.pop())
    }

    async fn tracked(&self, only: Option<String>) -> Result<Vec<TrackedMediaFile>> {
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                let mut query = media_file::table.into_boxed();
                if let Some(id) = only {
                    query = query.filter(media_file::id.eq(id));
                }
                let rows: Vec<(String, String, String, i64)> = query
                    .select((
                        media_file::id,
                        media_file::source_key,
                        media_file::state,
                        media_file::state_since_ms,
                    ))
                    .load(conn)?;
                rows.into_iter()
                    .map(|(id, source_key, state, since)| {
                        let state = MediaFileState::parse(&state)?;
                        Ok(TrackedMediaFile {
                            id,
                            source_key,
                            state,
                            overdue: state == MediaFileState::Processing
                                && now - since > PROCESSING_TIMEOUT_MS,
                        })
                    })
                    .collect()
            })
            .await
    }

    /// The state of Media file `id`, if it exists.
    pub async fn media_file_status(&self, id: &str) -> Result<Option<MediaFileStatus>> {
        let id = id.to_string();
        self.db
            .run(move |conn| {
                let row: Option<(String, String, Option<String>)> = media_file::table
                    .find(&id)
                    .select((media_file::id, media_file::state, media_file::error))
                    .first(conn)
                    .optional()?;
                row.map(|(id, state, error)| {
                    Ok(MediaFileStatus {
                        id,
                        state: MediaFileState::parse(&state)?,
                        error,
                    })
                })
                .transpose()
            })
            .await
    }

    /// Media file `id`'s transcode did not complete: a Processing one is now
    /// Failed with `error`. Any other state stays, so that a late hint about
    /// an earlier version does not undo a Ready one.
    pub async fn transcode_failed(&self, id: &str, error: &str) -> Result<()> {
        self.set_state(id, &[PROCESSING], FAILED, Some(error.to_string()))
            .await
    }

    /// The output of Ready Media file `id` is gone from `final`: it is
    /// Missing until it comes back. It is never deleted automatically.
    pub async fn output_lost(&self, id: &str) -> Result<()> {
        self.set_state(id, &[READY], MISSING, None).await
    }

    /// An output marker in `final` for a key the catalogue never saw an
    /// upload for: the Media file is placed from that key, and Ready.
    pub async fn output_seen(&self, key: &str, manifests: Manifests) -> Result<()> {
        self.upload_seen(key).await?;
        self.output_found(&dodoglass_ids::task_id_from_key(key), manifests)
            .await
    }

    async fn set_state(
        &self,
        id: &str,
        from: &'static [&'static str],
        to: &'static str,
        error: Option<String>,
    ) -> Result<()> {
        let id = id.to_string();
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                diesel::update(media_file::table.find(&id))
                    .filter(media_file::state.eq_any(from))
                    .set((
                        media_file::state.eq(to),
                        media_file::state_since_ms.eq(now),
                        media_file::error.eq(error),
                    ))
                    .execute(conn)?;
                Ok(())
            })
            .await
    }

    /// The output marker of Media file `id` was found: it is Ready, with
    /// these manifests. A Failed or Missing one becomes Ready too: the marker
    /// is the source of truth (ADR 0001).
    pub async fn output_found(&self, id: &str, manifests: Manifests) -> Result<()> {
        let id = id.to_string();
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                diesel::update(media_file::table.find(&id))
                    .filter(media_file::state.ne(READY))
                    .set((
                        media_file::state.eq(READY),
                        media_file::state_since_ms.eq(now),
                        media_file::hls_manifest.eq(manifests.hls),
                        media_file::dash_manifest.eq(manifests.dash),
                        media_file::error.eq(None::<String>),
                    ))
                    .execute(conn)?;
                Ok(())
            })
            .await
    }
}
