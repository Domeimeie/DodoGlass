//! Show, Season and Episode metadata: what TMDB lookups found, and Locked
//! fields.
//!
//! Shows are looked up as Movies are (`super::metadata`): a Match, Needs
//! review with candidates, or a retry while TMDB is unavailable. Once a Show
//! has a Match, each of its Seasons and Episodes is looked up by number,
//! without a search or review of its own: one TMDB does not know is
//! `not_found` and keeps its number as its name. No metadata row = not
//! looked up yet, so a Season or Episode that appears later under a matched
//! Show is picked up by the next pass.

use std::time::Duration;

use anyhow::Result;
use diesel::prelude::*;
use serde::Serialize;

use super::Catalogue;
use super::metadata::{Candidate, LockedFields};
use super::schema::{
    episode, episode_metadata, season, season_metadata, show, show_candidate, show_metadata,
};

const MATCHED: &str = "matched";
const NEEDS_REVIEW: &str = "needs_review";
const RETRYING: &str = "retrying";
const NOT_FOUND: &str = "not_found";

/// The fields an admin can lock on a Show (as stored in `locked_fields`).
pub const SHOW_LOCKABLE_FIELDS: [&str; 5] = ["name", "year", "imdb_id", "description", "image"];
/// The fields an admin can lock on a Season.
pub const SEASON_LOCKABLE_FIELDS: [&str; 3] = ["name", "description", "image"];
/// The fields an admin can lock on an Episode.
pub const EPISODE_LOCKABLE_FIELDS: [&str; 5] =
    ["name", "description", "image", "air_date", "imdb_id"];

/// A Show waiting for a TMDB lookup.
#[derive(Debug, Clone)]
pub struct PendingShowLookup {
    pub show_id: i32,
    /// Name and first-air year from the upload folder: what TMDB is searched for.
    pub name: String,
    pub year: i32,
    /// Lookups that already failed because TMDB was unavailable.
    pub failed_attempts: i32,
}

/// What a Show's Match fills in.
#[derive(Debug, Clone)]
pub struct ShowMatch {
    pub tmdb_id: i64,
    pub name: String,
    pub year: i32,
    pub imdb_id: Option<String>,
    pub description: Option<String>,
    /// Key of the downloaded poster in `assets`.
    pub image_key: Option<String>,
}

/// A Season or Episode of a matched Show, waiting to be looked up by number.
#[derive(Debug, Clone)]
pub struct PendingPart {
    pub part: Part,
    /// The Show it belongs to.
    pub show_id: i32,
    /// The TMDB id of its Show's Match.
    pub show_tmdb_id: i64,
    pub season: i32,
    /// Lookups that already failed because TMDB was unavailable.
    pub failed_attempts: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Part {
    Season { season_id: i32 },
    Episode { episode_id: i32, number: i32 },
}

/// What TMDB knows of a Season.
#[derive(Debug, Clone, Default)]
pub struct SeasonMatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub image_key: Option<String>,
}

/// What TMDB knows of an Episode.
#[derive(Debug, Clone, Default)]
pub struct EpisodeMatch {
    pub tmdb_id: i64,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Key of the downloaded still in `assets`.
    pub image_key: Option<String>,
    pub air_date: Option<String>,
    pub imdb_id: Option<String>,
}

/// A Show in Needs review, as the admin API lists it.
#[derive(Debug, Clone, Serialize)]
pub struct NeedsReviewShow {
    pub id: i32,
    pub name: String,
    pub year: i32,
    pub candidates: Vec<Candidate>,
}

/// The metadata part of a Show in the read API; absent until a Match or an
/// admin's edit.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ShowMetadataView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Key of the poster, served at `GET /images/<key>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = show_metadata)]
pub(super) struct ShowMetadataRow {
    pub show_id: i32,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub description: Option<String>,
    pub image_key: Option<String>,
}

impl From<ShowMetadataRow> for ShowMetadataView {
    fn from(row: ShowMetadataRow) -> Self {
        ShowMetadataView {
            tmdb_id: row.tmdb_id,
            imdb_id: row.imdb_id,
            description: row.description,
            image: row.image_key,
        }
    }
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = season_metadata)]
pub(super) struct SeasonMetadataRow {
    pub season_id: i32,
    pub name: Option<String>,
    pub description: Option<String>,
    pub image_key: Option<String>,
    pub locked_fields: String,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = episode_metadata)]
pub(super) struct EpisodeMetadataRow {
    pub episode_id: i32,
    pub tmdb_id: Option<i64>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub image_key: Option<String>,
    pub air_date: Option<String>,
    pub imdb_id: Option<String>,
    pub locked_fields: String,
}

/// The metadata of these Shows: what a Match filled in, and fields an admin
/// edited (also before a Match).
pub(super) fn show_rows(conn: &mut SqliteConnection, ids: &[i32]) -> Result<Vec<ShowMetadataRow>> {
    Ok(show_metadata::table
        .filter(show_metadata::show_id.eq_any(ids))
        .select(ShowMetadataRow::as_select())
        .load(conn)?)
}

/// The metadata of these Seasons (looked up, found or not).
pub(super) fn season_rows(
    conn: &mut SqliteConnection,
    ids: &[i32],
) -> Result<Vec<SeasonMetadataRow>> {
    Ok(season_metadata::table
        .filter(season_metadata::season_id.eq_any(ids))
        .select(SeasonMetadataRow::as_select())
        .load(conn)?)
}

/// The metadata of these Episodes (looked up, found or not).
pub(super) fn episode_rows(
    conn: &mut SqliteConnection,
    ids: &[i32],
) -> Result<Vec<EpisodeMetadataRow>> {
    Ok(episode_metadata::table
        .filter(episode_metadata::episode_id.eq_any(ids))
        .select(EpisodeMetadataRow::as_select())
        .load(conn)?)
}

/// Makes every Season and Episode of Show `show_id` due for a lookup by
/// number. With `clear`, what they had from TMDB is cleared first, except
/// their Locked fields.
fn look_up_parts_again(conn: &mut SqliteConnection, show_id: i32, clear: bool) -> Result<()> {
    let seasons: Vec<i32> = season::table
        .filter(season::show_id.eq(show_id))
        .select(season::id)
        .load(conn)?;
    let episodes: Vec<i32> = episode::table
        .filter(episode::season_id.eq_any(&seasons))
        .select(episode::id)
        .load(conn)?;
    if clear {
        for c in season_rows(conn, &seasons)? {
            let locks = LockedFields::parse(&c.locked_fields);
            diesel::update(season_metadata::table.find(c.season_id))
                .set((
                    season_metadata::name.eq(unless_locked(&locks, "name", None, c.name)),
                    season_metadata::description.eq(unless_locked(
                        &locks,
                        "description",
                        None,
                        c.description,
                    )),
                    season_metadata::image_key.eq(unless_locked(
                        &locks,
                        "image",
                        None,
                        c.image_key,
                    )),
                ))
                .execute(conn)?;
        }
        for c in episode_rows(conn, &episodes)? {
            let locks = LockedFields::parse(&c.locked_fields);
            diesel::update(episode_metadata::table.find(c.episode_id))
                .set((
                    episode_metadata::tmdb_id.eq(None::<i64>),
                    episode_metadata::name.eq(unless_locked(&locks, "name", None, c.name)),
                    episode_metadata::description.eq(unless_locked(
                        &locks,
                        "description",
                        None,
                        c.description,
                    )),
                    episode_metadata::image_key.eq(unless_locked(
                        &locks,
                        "image",
                        None,
                        c.image_key,
                    )),
                    episode_metadata::air_date
                        .eq(unless_locked(&locks, "air_date", None, c.air_date)),
                    episode_metadata::imdb_id.eq(unless_locked(&locks, "imdb_id", None, c.imdb_id)),
                ))
                .execute(conn)?;
        }
    }
    diesel::update(season_metadata::table.filter(season_metadata::season_id.eq_any(&seasons)))
        .set((
            season_metadata::match_state.eq(RETRYING),
            season_metadata::lookup_attempts.eq(0),
            season_metadata::retry_at_ms.eq(Some(0i64)),
        ))
        .execute(conn)?;
    diesel::update(episode_metadata::table.filter(episode_metadata::episode_id.eq_any(&episodes)))
        .set((
            episode_metadata::match_state.eq(RETRYING),
            episode_metadata::lookup_attempts.eq(0),
            episode_metadata::retry_at_ms.eq(Some(0i64)),
        ))
        .execute(conn)?;
    Ok(())
}

/// `new`, unless `field` is locked: then `old`.
fn unless_locked<T>(locks: &LockedFields, field: &str, new: T, old: T) -> T {
    if locks.contains(field) { old } else { new }
}

impl Catalogue {
    /// The Shows to look up now: new ones, and those whose retry is due.
    pub async fn pending_show_lookups(&self) -> Result<Vec<PendingShowLookup>> {
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                let rows: Vec<(i32, String, i32, Option<i32>)> = show::table
                    .left_join(show_metadata::table)
                    .filter(
                        show_metadata::show_id
                            .is_null()
                            .or(show_metadata::match_state
                                .eq(RETRYING)
                                .and(show_metadata::retry_at_ms.le(now))),
                    )
                    .order(show::id.asc())
                    .select((
                        show::id,
                        show::folder_name,
                        show::folder_year,
                        show_metadata::lookup_attempts.nullable(),
                    ))
                    .load(conn)?;
                Ok(rows
                    .into_iter()
                    .map(|(show_id, name, year, attempts)| PendingShowLookup {
                        show_id,
                        name,
                        year,
                        failed_attempts: attempts.unwrap_or(0),
                    })
                    .collect())
            })
            .await
    }

    /// The Show has a Match: every field that is not locked is filled in.
    /// Its Seasons and Episodes are looked up again by the next
    /// [`Self::pending_parts`]; when the Match is another TMDB Show than
    /// before, what they had from the old one is cleared first (Locked fields
    /// are kept), so nothing of it stays should TMDB not answer.
    pub async fn show_matched(&self, show_id: i32, found: ShowMatch) -> Result<()> {
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let current: Option<(String, ShowMetadataRow)> = show_metadata::table
                        .find(show_id)
                        .select((show_metadata::locked_fields, ShowMetadataRow::as_select()))
                        .first(conn)
                        .optional()?;
                    let (locks, current) = match current {
                        Some((locks, c)) => (LockedFields::parse(&locks), Some(c)),
                        None => (LockedFields::default(), None),
                    };
                    let same_match = current
                        .as_ref()
                        .is_some_and(|c| c.tmdb_id == Some(found.tmdb_id));
                    let (imdb_id, description, image_key) = match current {
                        Some(c) => (
                            unless_locked(&locks, "imdb_id", found.imdb_id, c.imdb_id),
                            unless_locked(&locks, "description", found.description, c.description),
                            unless_locked(&locks, "image", found.image_key, c.image_key),
                        ),
                        None => (found.imdb_id, found.description, found.image_key),
                    };
                    if !locks.contains("name") {
                        diesel::update(show::table.find(show_id))
                            .set(show::name.eq(&found.name))
                            .execute(conn)?;
                    }
                    if !locks.contains("year") {
                        diesel::update(show::table.find(show_id))
                            .set(show::year.eq(found.year))
                            .execute(conn)?;
                    }
                    let values = (
                        show_metadata::match_state.eq(MATCHED),
                        show_metadata::lookup_attempts.eq(0),
                        show_metadata::retry_at_ms.eq(None::<i64>),
                        show_metadata::tmdb_id.eq(Some(found.tmdb_id)),
                        show_metadata::imdb_id.eq(imdb_id),
                        show_metadata::description.eq(description),
                        show_metadata::image_key.eq(image_key),
                    );
                    diesel::insert_into(show_metadata::table)
                        .values((show_metadata::show_id.eq(show_id), values.clone()))
                        .on_conflict(show_metadata::show_id)
                        .do_update()
                        .set(values)
                        .execute(conn)?;
                    diesel::delete(
                        show_candidate::table.filter(show_candidate::show_id.eq(show_id)),
                    )
                    .execute(conn)?;
                    look_up_parts_again(conn, show_id, !same_match)
                })
            })
            .await
    }

    /// No certain Match: the Show is in Needs review, with these candidates
    /// (best first). It keeps the name and year from its folder, and its
    /// Seasons and Episodes their numbers as names.
    pub async fn show_needs_review(&self, show_id: i32, candidates: Vec<Candidate>) -> Result<()> {
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let values = (
                        show_metadata::match_state.eq(NEEDS_REVIEW),
                        show_metadata::lookup_attempts.eq(0),
                        show_metadata::retry_at_ms.eq(None::<i64>),
                    );
                    diesel::insert_into(show_metadata::table)
                        .values((show_metadata::show_id.eq(show_id), values))
                        .on_conflict(show_metadata::show_id)
                        .do_update()
                        .set(values)
                        .execute(conn)?;
                    diesel::delete(
                        show_candidate::table.filter(show_candidate::show_id.eq(show_id)),
                    )
                    .execute(conn)?;
                    let rows: Vec<_> = candidates
                        .into_iter()
                        .enumerate()
                        .map(|(i, c)| {
                            (
                                show_candidate::show_id.eq(show_id),
                                show_candidate::position.eq(i as i32),
                                show_candidate::tmdb_id.eq(c.tmdb_id),
                                show_candidate::name.eq(c.name),
                                show_candidate::year.eq(c.year),
                                show_candidate::description.eq(c.description),
                                show_candidate::poster_path.eq(c.poster_path),
                            )
                        })
                        .collect();
                    diesel::insert_into(show_candidate::table)
                        .values(rows)
                        .execute(conn)?;
                    Ok(())
                })
            })
            .await
    }

    /// TMDB was unavailable: look the Show up again in `retry_in`.
    pub async fn show_lookup_failed(
        &self,
        show_id: i32,
        failed_attempts: i32,
        retry_in: Duration,
    ) -> Result<()> {
        let retry_at_ms = self.clock.now_ms() + retry_in.as_millis() as i64;
        self.db
            .run(move |conn| {
                let values = (
                    show_metadata::match_state.eq(RETRYING),
                    show_metadata::lookup_attempts.eq(failed_attempts),
                    show_metadata::retry_at_ms.eq(Some(retry_at_ms)),
                );
                diesel::insert_into(show_metadata::table)
                    .values((show_metadata::show_id.eq(show_id), values))
                    .on_conflict(show_metadata::show_id)
                    .do_update()
                    .set(values)
                    .execute(conn)?;
                Ok(())
            })
            .await
    }

    /// The Shows in Needs review, by name, each with its candidates.
    pub async fn shows_needing_review(&self) -> Result<Vec<NeedsReviewShow>> {
        self.db
            .run(|conn| {
                let shows: Vec<(i32, String, i32)> = show::table
                    .inner_join(show_metadata::table)
                    .filter(show_metadata::match_state.eq(NEEDS_REVIEW))
                    .order((show::name.asc(), show::year.asc(), show::id.asc()))
                    .select((show::id, show::name, show::year))
                    .load(conn)?;
                let ids: Vec<i32> = shows.iter().map(|(id, _, _)| *id).collect();
                #[allow(clippy::type_complexity)]
                let candidates: Vec<(
                    i32,
                    i64,
                    String,
                    Option<i32>,
                    Option<String>,
                    Option<String>,
                )> = show_candidate::table
                    .filter(show_candidate::show_id.eq_any(&ids))
                    .order((show_candidate::show_id, show_candidate::position))
                    .select((
                        show_candidate::show_id,
                        show_candidate::tmdb_id,
                        show_candidate::name,
                        show_candidate::year,
                        show_candidate::description,
                        show_candidate::poster_path,
                    ))
                    .load(conn)?;
                Ok(shows
                    .into_iter()
                    .map(|(id, name, year)| NeedsReviewShow {
                        id,
                        name,
                        year,
                        candidates: candidates
                            .iter()
                            .filter(|c| c.0 == id)
                            .map(|c| Candidate {
                                tmdb_id: c.1,
                                name: c.2.clone(),
                                year: c.3,
                                description: c.4.clone(),
                                poster_path: c.5.clone(),
                            })
                            .collect(),
                    })
                    .collect())
            })
            .await
    }

    /// The Seasons and Episodes of matched Shows to look up now: those not
    /// looked up yet, and those whose retry is due. Seasons first.
    pub async fn pending_parts(&self) -> Result<Vec<PendingPart>> {
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                #[allow(clippy::type_complexity)]
                let seasons: Vec<(i32, i32, i32, Option<i64>, Option<i32>)> = season::table
                    .inner_join(show::table.inner_join(show_metadata::table))
                    .left_join(season_metadata::table)
                    .filter(show_metadata::match_state.eq(MATCHED))
                    .filter(
                        season_metadata::season_id
                            .is_null()
                            .or(season_metadata::match_state
                                .eq(RETRYING)
                                .and(season_metadata::retry_at_ms.le(now))),
                    )
                    .order(season::id.asc())
                    .select((
                        season::id,
                        season::show_id,
                        season::number,
                        show_metadata::tmdb_id,
                        season_metadata::lookup_attempts.nullable(),
                    ))
                    .load(conn)?;
                #[allow(clippy::type_complexity)]
                let episodes: Vec<(i32, i32, i32, i32, Option<i64>, Option<i32>)> = episode::table
                    .inner_join(
                        season::table.inner_join(show::table.inner_join(show_metadata::table)),
                    )
                    .left_join(episode_metadata::table)
                    .filter(show_metadata::match_state.eq(MATCHED))
                    .filter(
                        episode_metadata::episode_id
                            .is_null()
                            .or(episode_metadata::match_state
                                .eq(RETRYING)
                                .and(episode_metadata::retry_at_ms.le(now))),
                    )
                    .order(episode::id.asc())
                    .select((
                        episode::id,
                        episode::number,
                        season::show_id,
                        season::number,
                        show_metadata::tmdb_id,
                        episode_metadata::lookup_attempts.nullable(),
                    ))
                    .load(conn)?;
                let seasons = seasons.into_iter().filter_map(
                    |(season_id, show_id, number, tmdb, attempts)| {
                        Some(PendingPart {
                            part: Part::Season { season_id },
                            show_id,
                            show_tmdb_id: tmdb?,
                            season: number,
                            failed_attempts: attempts.unwrap_or(0),
                        })
                    },
                );
                let episodes = episodes.into_iter().filter_map(
                    |(episode_id, number, show_id, season, tmdb, attempts)| {
                        Some(PendingPart {
                            part: Part::Episode { episode_id, number },
                            show_id,
                            show_tmdb_id: tmdb?,
                            season,
                            failed_attempts: attempts.unwrap_or(0),
                        })
                    },
                );
                Ok(seasons.chain(episodes).collect())
            })
            .await
    }

    /// The Season was looked up: `found` is what TMDB knows of it, `None` if
    /// it does not know it (the number stays its name). Locked fields are kept.
    pub async fn season_looked_up(&self, season_id: i32, found: Option<SeasonMatch>) -> Result<()> {
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let state = if found.is_some() { MATCHED } else { NOT_FOUND };
                    let found = found.unwrap_or_default();
                    let current = season_rows(conn, &[season_id])?.pop();
                    let (name, description, image_key) = match current {
                        Some(c) => {
                            let locks = LockedFields::parse(&c.locked_fields);
                            (
                                unless_locked(&locks, "name", found.name, c.name),
                                unless_locked(
                                    &locks,
                                    "description",
                                    found.description,
                                    c.description,
                                ),
                                unless_locked(&locks, "image", found.image_key, c.image_key),
                            )
                        }
                        None => (found.name, found.description, found.image_key),
                    };
                    let values = (
                        season_metadata::match_state.eq(state),
                        season_metadata::lookup_attempts.eq(0),
                        season_metadata::retry_at_ms.eq(None::<i64>),
                        season_metadata::name.eq(name),
                        season_metadata::description.eq(description),
                        season_metadata::image_key.eq(image_key),
                    );
                    diesel::insert_into(season_metadata::table)
                        .values((season_metadata::season_id.eq(season_id), values.clone()))
                        .on_conflict(season_metadata::season_id)
                        .do_update()
                        .set(values)
                        .execute(conn)?;
                    Ok(())
                })
            })
            .await
    }

    /// The Episode was looked up: `found` is what TMDB knows of it, `None` if
    /// it does not know it (the number stays its name). Locked fields are kept.
    pub async fn episode_looked_up(
        &self,
        episode_id: i32,
        found: Option<EpisodeMatch>,
    ) -> Result<()> {
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let state = if found.is_some() { MATCHED } else { NOT_FOUND };
                    let found = found.unwrap_or_default();
                    let tmdb_id = (state == MATCHED).then_some(found.tmdb_id);
                    let current = episode_rows(conn, &[episode_id])?.pop();
                    let (name, description, image_key, air_date, imdb_id) = match current {
                        Some(c) => {
                            let locks = LockedFields::parse(&c.locked_fields);
                            (
                                unless_locked(&locks, "name", found.name, c.name),
                                unless_locked(
                                    &locks,
                                    "description",
                                    found.description,
                                    c.description,
                                ),
                                unless_locked(&locks, "image", found.image_key, c.image_key),
                                unless_locked(&locks, "air_date", found.air_date, c.air_date),
                                unless_locked(&locks, "imdb_id", found.imdb_id, c.imdb_id),
                            )
                        }
                        None => (
                            found.name,
                            found.description,
                            found.image_key,
                            found.air_date,
                            found.imdb_id,
                        ),
                    };
                    let values = (
                        episode_metadata::match_state.eq(state),
                        episode_metadata::lookup_attempts.eq(0),
                        episode_metadata::retry_at_ms.eq(None::<i64>),
                        episode_metadata::tmdb_id.eq(tmdb_id),
                        episode_metadata::name.eq(name),
                        episode_metadata::description.eq(description),
                        episode_metadata::image_key.eq(image_key),
                        episode_metadata::air_date.eq(air_date),
                        episode_metadata::imdb_id.eq(imdb_id),
                    );
                    diesel::insert_into(episode_metadata::table)
                        .values((episode_metadata::episode_id.eq(episode_id), values.clone()))
                        .on_conflict(episode_metadata::episode_id)
                        .do_update()
                        .set(values)
                        .execute(conn)?;
                    Ok(())
                })
            })
            .await
    }

    /// TMDB was unavailable: look the Season or Episode up again in `retry_in`.
    pub async fn part_lookup_failed(
        &self,
        part: Part,
        failed_attempts: i32,
        retry_in: Duration,
    ) -> Result<()> {
        let retry_at_ms = self.clock.now_ms() + retry_in.as_millis() as i64;
        self.db
            .run(move |conn| {
                match part {
                    Part::Season { season_id } => {
                        let values = (
                            season_metadata::match_state.eq(RETRYING),
                            season_metadata::lookup_attempts.eq(failed_attempts),
                            season_metadata::retry_at_ms.eq(Some(retry_at_ms)),
                        );
                        diesel::insert_into(season_metadata::table)
                            .values((season_metadata::season_id.eq(season_id), values))
                            .on_conflict(season_metadata::season_id)
                            .do_update()
                            .set(values)
                            .execute(conn)?;
                    }
                    Part::Episode { episode_id, .. } => {
                        let values = (
                            episode_metadata::match_state.eq(RETRYING),
                            episode_metadata::lookup_attempts.eq(failed_attempts),
                            episode_metadata::retry_at_ms.eq(Some(retry_at_ms)),
                        );
                        diesel::insert_into(episode_metadata::table)
                            .values((episode_metadata::episode_id.eq(episode_id), values))
                            .on_conflict(episode_metadata::episode_id)
                            .do_update()
                            .set(values)
                            .execute(conn)?;
                    }
                }
                Ok(())
            })
            .await
    }
}
