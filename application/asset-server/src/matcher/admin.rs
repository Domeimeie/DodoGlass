//! Lookups an admin asks for: picking a Match (one of the candidates or any
//! TMDB id), refreshing an entry, and attaching an Unassigned media file to a
//! new Movie or Show created from a TMDB id. Locked fields are kept, as in
//! every lookup.

use anyhow::Result;

use super::{LookupError, Matcher};

/// What came of an admin's lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Corrected {
    Done,
    /// No such entry (or Unassigned media file); the message says which.
    NotFound(String),
    /// TMDB refused: no such id, or nothing usable under it.
    Rejected(String),
    /// TMDB (or the `assets` bucket) is unavailable: try again later.
    Unavailable(String),
}

impl From<LookupError> for Corrected {
    fn from(e: LookupError) -> Self {
        match e {
            LookupError::Retry(e) => Corrected::Unavailable(e),
            LookupError::Review(e) => Corrected::Rejected(e),
        }
    }
}

impl Matcher {
    /// Makes TMDB Movie `tmdb_id` the Match of Movie `movie_id`: every field
    /// that is not locked is refilled, and it leaves Needs review.
    pub async fn pick_movie_match(&self, movie_id: i32, tmdb_id: i64) -> Result<Corrected> {
        let Some(movie) = self.catalogue.movie_lookup(movie_id).await? else {
            return Ok(Corrected::NotFound("no such movie".into()));
        };
        let found = match self.fetch_movie(tmdb_id, Some(movie.lookup.year)).await {
            Ok(found) => found,
            Err(e) => return Ok(e.into()),
        };
        self.catalogue.movie_matched(movie_id, found).await?;
        Ok(Corrected::Done)
    }
}
