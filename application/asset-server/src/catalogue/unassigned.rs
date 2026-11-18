//! Unassigned media files: uploads whose path does not follow the upload
//! layout (ADR 0003). They are kept and transcoded like any other Media file,
//! but belong to no Movie or Episode until an admin attaches them.

use anyhow::Result;
use diesel::prelude::*;
use dodoglass_ids::{Placement, UnassignedReason, place_upload};
use serde::Serialize;

use super::Catalogue;
use super::schema::media_file;

/// An Unassigned media file, as the admin API lists it.
#[derive(Debug, Clone, Serialize)]
pub struct UnassignedView {
    pub id: String,
    /// The upload key in `ingest`.
    pub path: String,
    /// Why the path does not fit the upload layout (`no_year`, ...).
    pub reason: Option<&'static str>,
    pub state: String,
}

/// Why `key` does not follow the upload layout, as the admin API names it.
/// The reason is taken from the path each time, so it needs no column.
fn reason(key: &str) -> Option<&'static str> {
    use UnassignedReason::*;
    match place_upload(key) {
        Placement::Unassigned(reason) => Some(match reason {
            NoYear => "no_year",
            UnknownRootFolder => "unknown_root_folder",
            WrongDepth => "wrong_depth",
            UnsupportedExtension => "unsupported_extension",
            UnknownSeasonFolder => "unknown_season_folder",
            NoEpisodeCode => "no_episode_code",
            SeasonMismatch => "season_mismatch",
        }),
        // Only if the layout rules changed since the upload was placed.
        Placement::Movie { .. } | Placement::Episode { .. } => None,
    }
}

impl Catalogue {
    /// The Unassigned media files, by upload path, whatever their state.
    pub async fn unassigned_media_files(&self) -> Result<Vec<UnassignedView>> {
        self.db
            .run(|conn| {
                let rows: Vec<(String, String, String)> = media_file::table
                    .filter(media_file::movie_id.is_null())
                    .filter(media_file::episode_id.is_null())
                    .order(media_file::source_key.asc())
                    .select((media_file::id, media_file::source_key, media_file::state))
                    .load(conn)?;
                Ok(rows
                    .into_iter()
                    .map(|(id, path, state)| UnassignedView {
                        id,
                        reason: reason(&path),
                        path,
                        state,
                    })
                    .collect())
            })
            .await
    }
}
