//! The rating sweep: keeps Ratings current (ADR 0005). Everything else from
//! TMDB is fetched once; a Rating changes all the time, so each sweep fetches
//! the Ratings older than a maximum age again, oldest first, a batch at a
//! time. A Movie or Show costs one request (its details), the Episodes of a
//! Season one together (the Season's details list every Episode's score).
//!
//! A sweep writes only Ratings: names, descriptions, images and Locked fields
//! stay as they are. TMDB unavailable ends the sweep: the Ratings keep their
//! value and timestamp, so they are still the oldest and are tried first by
//! the next sweep (an hour later by default, as the lookups' longest retry);
//! nothing goes to Needs review. TMDB refusing (it no longer knows the
//! entry) keeps the Rating too, and records the attempt so that the entry
//! waits a maximum age rather than blocking every batch.

use std::time::Duration;

use anyhow::Result;
use tracing::{info, warn};

use super::Matcher;
use crate::catalogue::StaleRating;
use crate::ports::tmdb::TmdbError;

/// Ratings older than this are fetched again: 7 days.
pub const RATING_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// How often `main` runs a sweep: hourly.
pub const RATING_SWEEP_INTERVAL: Duration = Duration::from_secs(3600);

/// At most this many entries per sweep (a Season's Episodes count as one).
pub const RATING_SWEEP_BATCH: usize = 20;

impl Matcher {
    /// One rating sweep: fetches up to `batch` of the Ratings older than
    /// `max_age` again, oldest first (never fetched before first of all).
    pub async fn sweep_ratings(&self, max_age: Duration, batch: usize) -> Result<()> {
        let stale = self.catalogue.stale_ratings(max_age, batch).await?;
        let mut refreshed = 0;
        for entry in stale {
            match self.refresh_rating(&entry).await? {
                Ok(()) => refreshed += 1,
                Err(TmdbError::Unavailable(e)) => {
                    warn!(?entry, error = %e, refreshed, "TMDB unavailable, rating sweep stopped until the next one");
                    return Ok(());
                }
                Err(TmdbError::Rejected(e)) => {
                    warn!(?entry, error = %e, "TMDB refused, keeping the Rating");
                    self.catalogue.rating_kept(entry).await?;
                }
            }
        }
        if refreshed > 0 {
            info!(refreshed, "ratings refreshed");
        }
        Ok(())
    }

    /// Fetches the Rating of `entry` and stores it; `Err` with what TMDB
    /// answered when it did not give one.
    async fn refresh_rating(&self, entry: &StaleRating) -> Result<Result<(), TmdbError>> {
        match *entry {
            StaleRating::Movie { movie_id, tmdb_id } => match self.tmdb.movie(tmdb_id).await {
                Ok(details) => {
                    let rating = details.rating;
                    self.catalogue
                        .movie_rating_fetched(movie_id, rating)
                        .await?;
                    Ok(Ok(()))
                }
                Err(e) => Ok(Err(e)),
            },
            StaleRating::Show { show_id, tmdb_id } => match self.tmdb.tv(tmdb_id).await {
                Ok(details) => {
                    let rating = details.rating;
                    self.catalogue.show_rating_fetched(show_id, rating).await?;
                    Ok(Ok(()))
                }
                Err(e) => Ok(Err(e)),
            },
            StaleRating::Season {
                season_id,
                show_tmdb_id,
                number,
            } => match self.tmdb.season(show_tmdb_id, number).await {
                Ok(Some(details)) => {
                    self.catalogue
                        .episode_ratings_fetched(season_id, details.episodes)
                        .await?;
                    Ok(Ok(()))
                }
                Ok(None) => Ok(Err(TmdbError::Rejected(format!(
                    "no season {number} of show {show_tmdb_id}"
                )))),
                Err(e) => Ok(Err(e)),
            },
        }
    }
}
