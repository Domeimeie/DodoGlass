//! Ratings: TMDB's user score and vote count of Movies, Shows and Episodes
//! (Seasons have none). Every Match and refresh sets the Rating with the
//! other fields; unlike them it is never a Locked field, and the rating
//! sweep (`crate::matcher`) keeps it current (ADR 0005). Each Rating records
//! when it was last fetched (`rating_updated_ms`).

use std::collections::BTreeMap;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Result;
use diesel::prelude::*;
use serde::Serialize;

use super::Catalogue;
use super::match_state::MatchState;
use super::schema::{episode, episode_metadata, movie_metadata, season, show, show_metadata};
use crate::ports::tmdb::{EpisodeRating, Rating};

/// The field names of a Rating in the API: none of them can be edited or
/// locked.
pub const RATING_FIELDS: [&str; 3] = ["rating", "votes", "rating_updated_at"];

/// The Rating part of a Movie, Show or Episode in the read API: always
/// there, `null` without a Rating.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RatingView {
    /// TMDB's score, 0–10.
    pub rating: Option<f64>,
    pub votes: Option<i64>,
    /// When the Rating was last fetched (RFC 3339, UTC).
    pub rating_updated_at: Option<String>,
}

impl RatingView {
    /// From the `rating`, `votes` and `rating_updated_ms` columns.
    pub(super) fn from_columns(
        rating: Option<f64>,
        votes: Option<i64>,
        updated_ms: Option<i64>,
    ) -> RatingView {
        RatingView {
            rating,
            votes,
            rating_updated_at: updated_ms.map(timestamp),
        }
    }
}

/// Epoch milliseconds as RFC 3339 in UTC, to the second:
/// `2026-11-17T00:00:00Z`.
fn timestamp(ms: i64) -> String {
    let at = UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap_or(0));
    humantime::format_rfc3339_seconds(at).to_string()
}

/// The `rating` and `votes` columns of `rating`.
pub(super) fn columns(rating: Option<Rating>) -> (Option<f64>, Option<i64>) {
    (rating.map(|r| r.score), rating.map(|r| r.votes))
}

/// An entry whose Rating the sweep fetches again. Only matched entries
/// have one: the others are never stale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StaleRating {
    Movie {
        movie_id: i32,
        tmdb_id: i64,
    },
    Show {
        show_id: i32,
        tmdb_id: i64,
    },
    /// The matched Episodes of a Season, fetched together from the Season's
    /// details (`number` of the TMDB Show `show_tmdb_id`).
    Season {
        season_id: i32,
        show_tmdb_id: i64,
        number: i32,
    },
}

/// A [`StaleRating`] with when its Rating was fetched (`None`: never), for
/// putting the oldest first.
type Aged = (Option<i64>, StaleRating);

/// Matched Movies with a Rating fetched before `cutoff` (or never), at most
/// `limit`, oldest first.
fn stale_movies(conn: &mut SqliteConnection, cutoff: i64, limit: i64) -> Result<Vec<Aged>> {
    let rows: Vec<(i32, Option<i64>, Option<i64>)> = movie_metadata::table
        .filter(movie_metadata::match_state.eq(MatchState::Matched))
        .filter(movie_metadata::tmdb_id.is_not_null())
        .filter(
            movie_metadata::rating_updated_ms
                .is_null()
                .or(movie_metadata::rating_updated_ms.lt(cutoff)),
        )
        // Never fetched first: portable, unlike NULLS FIRST.
        .order((
            movie_metadata::rating_updated_ms.is_not_null().asc(),
            movie_metadata::rating_updated_ms.asc(),
            movie_metadata::movie_id.asc(),
        ))
        .limit(limit)
        .select((
            movie_metadata::movie_id,
            movie_metadata::tmdb_id,
            movie_metadata::rating_updated_ms,
        ))
        .load(conn)?;
    Ok(rows
        .into_iter()
        .filter_map(|(movie_id, tmdb_id, at)| {
            Some((
                at,
                StaleRating::Movie {
                    movie_id,
                    tmdb_id: tmdb_id?,
                },
            ))
        })
        .collect())
}

/// As [`stale_movies`], for Shows.
fn stale_shows(conn: &mut SqliteConnection, cutoff: i64, limit: i64) -> Result<Vec<Aged>> {
    let rows: Vec<(i32, Option<i64>, Option<i64>)> = show_metadata::table
        .filter(show_metadata::match_state.eq(MatchState::Matched))
        .filter(show_metadata::tmdb_id.is_not_null())
        .filter(
            show_metadata::rating_updated_ms
                .is_null()
                .or(show_metadata::rating_updated_ms.lt(cutoff)),
        )
        .order((
            show_metadata::rating_updated_ms.is_not_null().asc(),
            show_metadata::rating_updated_ms.asc(),
            show_metadata::show_id.asc(),
        ))
        .limit(limit)
        .select((
            show_metadata::show_id,
            show_metadata::tmdb_id,
            show_metadata::rating_updated_ms,
        ))
        .load(conn)?;
    Ok(rows
        .into_iter()
        .filter_map(|(show_id, tmdb_id, at)| {
            Some((
                at,
                StaleRating::Show {
                    show_id,
                    tmdb_id: tmdb_id?,
                },
            ))
        })
        .collect())
}

/// The Seasons with a matched Episode whose Rating is older than `cutoff`
/// (or was never fetched), aged by their oldest such Episode, of Shows that
/// are matched.
fn stale_seasons(conn: &mut SqliteConnection, cutoff: i64) -> Result<Vec<Aged>> {
    let rows: Vec<(i32, i32, Option<i64>, Option<i64>)> = episode_metadata::table
        .inner_join(
            episode::table
                .inner_join(season::table.inner_join(show::table.inner_join(show_metadata::table))),
        )
        .filter(episode_metadata::match_state.eq(MatchState::Matched))
        .filter(episode_metadata::tmdb_id.is_not_null())
        .filter(show_metadata::match_state.eq(MatchState::Matched))
        .filter(
            episode_metadata::rating_updated_ms
                .is_null()
                .or(episode_metadata::rating_updated_ms.lt(cutoff)),
        )
        .select((
            season::id,
            season::number,
            show_metadata::tmdb_id,
            episode_metadata::rating_updated_ms,
        ))
        .load(conn)?;
    let mut seasons: BTreeMap<i32, Aged> = BTreeMap::new();
    for (season_id, number, show_tmdb_id, at) in rows {
        let Some(show_tmdb_id) = show_tmdb_id else {
            continue;
        };
        let stale = StaleRating::Season {
            season_id,
            show_tmdb_id,
            number,
        };
        let oldest = seasons.entry(season_id).or_insert((at, stale));
        oldest.0 = oldest.0.min(at);
    }
    Ok(seasons.into_values().collect())
}

/// Writes the Rating of the matched Episodes of Season `season_id` listed in
/// `ratings`, and `now` as when every matched Episode of it was fetched (one
/// TMDB no longer lists keeps its Rating, and waits a maximum age).
fn store_episode_ratings(
    conn: &mut SqliteConnection,
    season_id: i32,
    ratings: &[EpisodeRating],
    now: i64,
) -> Result<()> {
    let episodes: Vec<(i32, i32)> = episode::table
        .inner_join(episode_metadata::table)
        .filter(episode::season_id.eq(season_id))
        .filter(episode_metadata::match_state.eq(MatchState::Matched))
        .filter(episode_metadata::tmdb_id.is_not_null())
        .select((episode::id, episode::number))
        .load(conn)?;
    for (episode_id, number) in episodes {
        let row = episode_metadata::table.find(episode_id);
        match ratings.iter().find(|r| r.number == number) {
            Some(listed) => {
                let (rating, votes) = columns(listed.rating);
                diesel::update(row)
                    .set((
                        episode_metadata::rating.eq(rating),
                        episode_metadata::votes.eq(votes),
                        episode_metadata::rating_updated_ms.eq(Some(now)),
                    ))
                    .execute(conn)?;
            }
            None => {
                diesel::update(row)
                    .set(episode_metadata::rating_updated_ms.eq(Some(now)))
                    .execute(conn)?;
            }
        }
    }
    Ok(())
}

impl Catalogue {
    /// Up to `limit` entries whose Rating is older than `max_age` (or was
    /// never fetched), oldest first: matched Movies and Shows, and Seasons
    /// with such matched Episodes.
    pub async fn stale_ratings(&self, max_age: Duration, limit: usize) -> Result<Vec<StaleRating>> {
        let max_age = i64::try_from(max_age.as_millis()).unwrap_or(i64::MAX);
        let cutoff = self.clock.now_ms().saturating_sub(max_age);
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        self.db
            .run(move |conn| {
                let mut stale = stale_movies(conn, cutoff, limit_i64)?;
                stale.extend(stale_shows(conn, cutoff, limit_i64)?);
                stale.extend(stale_seasons(conn, cutoff)?);
                // Never fetched (None) sorts first; ties keep Movies, Shows,
                // Seasons in that order.
                stale.sort_by_key(|(at, _)| *at);
                Ok(stale.into_iter().take(limit).map(|(_, s)| s).collect())
            })
            .await
    }

    /// The sweep fetched Movie `movie_id`'s Rating: only the Rating changes.
    pub async fn movie_rating_fetched(&self, movie_id: i32, rating: Option<Rating>) -> Result<()> {
        let now = self.clock.now_ms();
        let (rating, votes) = columns(rating);
        self.db
            .run(move |conn| {
                diesel::update(movie_metadata::table.find(movie_id))
                    .set((
                        movie_metadata::rating.eq(rating),
                        movie_metadata::votes.eq(votes),
                        movie_metadata::rating_updated_ms.eq(Some(now)),
                    ))
                    .execute(conn)?;
                Ok(())
            })
            .await
    }

    /// The sweep fetched Show `show_id`'s Rating: only the Rating changes.
    pub async fn show_rating_fetched(&self, show_id: i32, rating: Option<Rating>) -> Result<()> {
        let now = self.clock.now_ms();
        let (rating, votes) = columns(rating);
        self.db
            .run(move |conn| {
                diesel::update(show_metadata::table.find(show_id))
                    .set((
                        show_metadata::rating.eq(rating),
                        show_metadata::votes.eq(votes),
                        show_metadata::rating_updated_ms.eq(Some(now)),
                    ))
                    .execute(conn)?;
                Ok(())
            })
            .await
    }

    /// The sweep fetched the details of Season `season_id`: its matched
    /// Episodes take their Ratings from `ratings`; nothing else changes.
    pub async fn episode_ratings_fetched(
        &self,
        season_id: i32,
        ratings: Vec<EpisodeRating>,
    ) -> Result<()> {
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                conn.transaction(|conn| store_episode_ratings(conn, season_id, &ratings, now))
            })
            .await
    }

    /// TMDB refused the sweep's request for `entry` (it no longer knows it):
    /// its Rating stays, and is tried again after a maximum age.
    pub async fn rating_kept(&self, entry: StaleRating) -> Result<()> {
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                match entry {
                    StaleRating::Movie { movie_id, .. } => {
                        diesel::update(movie_metadata::table.find(movie_id))
                            .set(movie_metadata::rating_updated_ms.eq(Some(now)))
                            .execute(conn)?;
                    }
                    StaleRating::Show { show_id, .. } => {
                        diesel::update(show_metadata::table.find(show_id))
                            .set(show_metadata::rating_updated_ms.eq(Some(now)))
                            .execute(conn)?;
                    }
                    StaleRating::Season { season_id, .. } => {
                        store_episode_ratings(conn, season_id, &[], now)?;
                    }
                }
                Ok(())
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339_in_utc() {
        assert_eq!(timestamp(1_794_873_600_000), "2026-11-17T00:00:00Z");
        assert_eq!(timestamp(1_794_873_601_999), "2026-11-17T00:00:01Z");
    }
}
