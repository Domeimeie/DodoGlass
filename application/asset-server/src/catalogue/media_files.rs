//! Media file states. A Media file is Processing from its upload until the
//! transcoder's output marker is found, then Ready (ADR 0001).

use anyhow::Result;
use diesel::prelude::*;
use diesel::upsert::excluded;
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
