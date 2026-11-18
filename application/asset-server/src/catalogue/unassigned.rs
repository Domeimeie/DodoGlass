//! Unassigned media files: uploads whose path does not follow the upload
//! layout (ADR 0003). They are kept and transcoded like any other Media file,
//! but belong to no Movie or Episode until an admin attaches them.

use anyhow::Result;
use diesel::prelude::*;
use dodoglass_ids::{Placement, UnassignedReason, place_upload};
use serde::{Deserialize, Serialize};

use super::match_state::MatchState;
use super::media_files::Owner;
use super::metadata::{MovieMatch, apply_movie_match};
use super::schema::{media_file, movie_metadata, show, show_metadata};
use super::show_metadata::{ShowMatch, apply_show_match};
use super::{Catalogue, media_files, movies, shows};

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

/// Whether `id` is an Unassigned media file.
fn is_unassigned(conn: &mut SqliteConnection, id: &str) -> Result<bool> {
    let count: i64 = media_file::table
        .find(id)
        .filter(media_file::movie_id.is_null())
        .filter(media_file::episode_id.is_null())
        .count()
        .get_result(conn)?;
    Ok(count > 0)
}

/// A year from TMDB as a folder year.
fn folder_year(year: i32) -> Result<u16> {
    u16::try_from(year).map_err(|_| anyhow::anyhow!("{year} is not a year"))
}

fn attached(label: Option<String>) -> Result<Attached> {
    Ok(match label {
        Some(label) => Attached::Attached { label },
        None => Attached::NoSuchMediaFile,
    })
}

/// Where an admin attaches an Unassigned media file: an existing Movie or
/// Episode, named as in the upload layout (the name and year of its folder).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachTo {
    Movie {
        name: String,
        year: u16,
    },
    Episode {
        show: String,
        year: u16,
        season: u32,
        number: u32,
    },
}

/// What came of attaching an Unassigned media file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attached {
    /// Attached, with this Label.
    Attached { label: String },
    /// No Unassigned media file has this id.
    NoSuchMediaFile,
    /// The Movie or Episode does not exist.
    NoSuchTarget,
}

impl Catalogue {
    /// Attaches the Unassigned media file `id` to an existing Movie or
    /// Episode. It gets its Label there by the same rule as an upload, and is
    /// listed there once it is Ready.
    pub async fn attach(&self, id: &str, to: AttachTo) -> Result<Attached> {
        let id = id.to_string();
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let owner = match &to {
                        AttachTo::Movie { name, year } => {
                            movies::find(conn, name, i32::from(*year))?.map(Owner::Movie)
                        }
                        AttachTo::Episode {
                            show,
                            year,
                            season,
                            number,
                        } => {
                            let place = shows::EpisodePlace {
                                show,
                                year: *year,
                                season: *season,
                                episode: *number,
                            };
                            shows::find(conn, place)?.map(Owner::Episode)
                        }
                    };
                    let Some(owner) = owner else {
                        return Ok(if is_unassigned(conn, &id)? {
                            Attached::NoSuchTarget
                        } else {
                            Attached::NoSuchMediaFile
                        });
                    };
                    Ok(match media_files::attach(conn, &id, owner)? {
                        Some(label) => Attached::Attached { label },
                        None => Attached::NoSuchMediaFile,
                    })
                })
            })
            .await
    }

    /// Whether `id` is an Unassigned media file.
    pub async fn is_unassigned_media_file(&self, id: &str) -> Result<bool> {
        let id = id.to_string();
        self.db.run(move |conn| is_unassigned(conn, &id)).await
    }

    /// Attaches the Unassigned media file `id` to the Movie TMDB knows as
    /// `found`, created with `found`'s name and year as its folder (so later
    /// uploads to `movies/<name> (<year>)/` join it) and `found` as its Match.
    /// A Movie with that folder that exists already is used, and gets the
    /// Match unless it has one.
    pub async fn attach_to_new_movie(&self, id: &str, found: MovieMatch) -> Result<Attached> {
        let id = id.to_string();
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    if !is_unassigned(conn, &id)? {
                        return Ok(Attached::NoSuchMediaFile);
                    }
                    let movie_id =
                        movies::find_or_create(conn, &found.name, folder_year(found.year)?)?;
                    let state: Option<MatchState> = movie_metadata::table
                        .find(movie_id)
                        .select(movie_metadata::match_state)
                        .first(conn)
                        .optional()?;
                    if state != Some(MatchState::Matched) {
                        apply_movie_match(conn, movie_id, found)?;
                    }
                    attached(media_files::attach(conn, &id, Owner::Movie(movie_id))?)
                })
            })
            .await
    }

    /// Attaches the Unassigned media file `id` to Episode `number` of Season
    /// `season` of the Show TMDB knows as `found`, created as for
    /// [`Self::attach_to_new_movie`] (with the Season and Episode if new).
    /// Answers the Show's id too, whose Seasons and Episodes are then due for
    /// a lookup by number.
    pub async fn attach_to_new_episode(
        &self,
        id: &str,
        found: ShowMatch,
        season: u32,
        number: u32,
    ) -> Result<(Attached, Option<i32>)> {
        let id = id.to_string();
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    if !is_unassigned(conn, &id)? {
                        return Ok((Attached::NoSuchMediaFile, None));
                    }
                    let year = folder_year(found.year)?;
                    let place = shows::EpisodePlace {
                        show: &found.name,
                        year,
                        season,
                        episode: number,
                    };
                    let episode_id = shows::find_or_create(conn, place)?;
                    let show_id: i32 = show::table
                        .filter(show::folder_name.eq(&found.name))
                        .filter(show::folder_year.eq(i32::from(year)))
                        .select(show::id)
                        .first(conn)?;
                    let state: Option<MatchState> = show_metadata::table
                        .find(show_id)
                        .select(show_metadata::match_state)
                        .first(conn)
                        .optional()?;
                    if state != Some(MatchState::Matched) {
                        apply_show_match(conn, show_id, found)?;
                    }
                    let owner = Owner::Episode(episode_id);
                    Ok((
                        attached(media_files::attach(conn, &id, owner)?)?,
                        Some(show_id),
                    ))
                })
            })
            .await
    }

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
