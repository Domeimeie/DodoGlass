//! Media file states. A Media file is Processing from its upload until the
//! transcoder's output marker is found, then Ready (ADR 0001).

use anyhow::Result;
use diesel::prelude::*;
use serde::Serialize;

use super::Catalogue;
use super::schema::media_file;

pub(super) const PROCESSING: &str = "processing";
pub(super) const READY: &str = "ready";

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
    /// The ids of the Media files waiting for their output marker.
    pub async fn processing_ids(&self) -> Result<Vec<String>> {
        self.db
            .run(|conn| {
                Ok(media_file::table
                    .filter(media_file::state.eq(PROCESSING))
                    .select(media_file::id)
                    .load(conn)?)
            })
            .await
    }

    /// The output marker of Media file `id` was found: it is Ready, with
    /// these manifests.
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
