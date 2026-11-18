//! Media file states. A Media file is Processing from its upload until the
//! transcoder's output marker is found, then Ready (ADR 0001). It is Failed
//! when its transcode did not complete, and Missing when the output of a
//! Ready one has disappeared from `final`.

use anyhow::Result;
use diesel::prelude::*;
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

/// What a Media file belongs to. Labels are unique, and there is one Default
/// media file, per owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    Movie(i32),
    Episode(i32),
}

/// Creates the Media file `id` as Processing, or makes an existing one
/// Processing again (a new version of the same upload: its owner, Label and
/// default flag stay).
///
/// A new Media file of `owner` is labelled `stem`, made unique within the
/// owner (see [`unique_label`]); the first one of an owner is its Default
/// media file. Run it inside the transaction that found the owner.
pub(super) fn processing(
    conn: &mut SqliteConnection,
    id: &str,
    key: &str,
    owner: Option<Owner>,
    stem: &str,
    now: i64,
) -> Result<()> {
    let updated = diesel::update(media_file::table.find(id))
        .set((
            media_file::state.eq(PROCESSING),
            media_file::state_since_ms.eq(now),
            media_file::hls_manifest.eq(None::<String>),
            media_file::dash_manifest.eq(None::<String>),
            media_file::error.eq(None::<String>),
        ))
        .execute(conn)?;
    if updated > 0 {
        return Ok(());
    }
    let (label, is_default) = match owner {
        Some(owner) => {
            let taken = labels_of(conn, owner)?;
            (unique_label(stem, extension(key), &taken), taken.is_empty())
        }
        None => (stem.to_string(), false),
    };
    let (movie, episode) = match owner {
        Some(Owner::Movie(movie)) => (Some(movie), None),
        Some(Owner::Episode(episode)) => (None, Some(episode)),
        None => (None, None),
    };
    diesel::insert_into(media_file::table)
        .values((
            media_file::id.eq(id),
            media_file::source_key.eq(key),
            media_file::movie_id.eq(movie),
            media_file::episode_id.eq(episode),
            media_file::label.eq(label),
            media_file::is_default.eq(is_default),
            media_file::state.eq(PROCESSING),
            media_file::state_since_ms.eq(now),
            media_file::created_ms.eq(now),
        ))
        .execute(conn)?;
    Ok(())
}

/// The Unassigned media file `id` now belongs to `owner`. Its Label (the file
/// name's stem) is made unique within the owner as for an upload, and it is
/// the owner's Default media file only if the owner had none. Returns its new
/// Label, or `None` if `id` is not an Unassigned media file. Run it inside a
/// transaction.
pub(super) fn attach(
    conn: &mut SqliteConnection,
    id: &str,
    owner: Owner,
) -> Result<Option<String>> {
    let found: Option<(String, String)> = media_file::table
        .find(id)
        .filter(media_file::movie_id.is_null())
        .filter(media_file::episode_id.is_null())
        .select((media_file::source_key, media_file::label))
        .first(conn)
        .optional()?;
    let Some((key, stem)) = found else {
        return Ok(None);
    };
    let taken = labels_of(conn, owner)?;
    let label = unique_label(&stem, extension(&key), &taken);
    let (movie, episode) = match owner {
        Owner::Movie(movie) => (Some(movie), None),
        Owner::Episode(episode) => (None, Some(episode)),
    };
    diesel::update(media_file::table.find(id))
        .set((
            media_file::movie_id.eq(movie),
            media_file::episode_id.eq(episode),
            media_file::label.eq(&label),
            media_file::is_default.eq(taken.is_empty()),
        ))
        .execute(conn)?;
    Ok(Some(label))
}

/// The Labels of every Media file of `owner`, whatever its state.
fn labels_of(conn: &mut SqliteConnection, owner: Owner) -> Result<Vec<String>> {
    let query = media_file::table.select(media_file::label);
    Ok(match owner {
        Owner::Movie(movie) => query.filter(media_file::movie_id.eq(movie)).load(conn)?,
        Owner::Episode(episode) => query
            .filter(media_file::episode_id.eq(episode))
            .load(conn)?,
    })
}

/// `stem` if no other Media file of the owner has it, else `stem (ext)`
/// (`matrix.mkv` + `matrix.mp4` → "matrix", "matrix (mp4)"), with a number
/// added if that is taken too.
fn unique_label(stem: &str, ext: &str, taken: &[String]) -> String {
    let free = |label: &String| !taken.contains(label);
    let mut candidates = std::iter::once(stem.to_string())
        .chain((!ext.is_empty()).then(|| format!("{stem} ({ext})")))
        .chain((2..).map(|n| match ext {
            "" => format!("{stem} ({n})"),
            ext => format!("{stem} ({ext} {n})"),
        }));
    candidates
        .find(free)
        .expect("the numbered candidates never end")
}

/// `movies/The Matrix (1999)/matrix.mkv` → `mkv`; empty without one.
fn extension(key: &str) -> &str {
    let name = key.rsplit('/').next().unwrap_or(key);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext,
        _ => "",
    }
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
