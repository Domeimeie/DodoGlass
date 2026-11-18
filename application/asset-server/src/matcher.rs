//! The matcher: looks new Movies up in TMDB, once, when they first appear.
//!
//! - **Strict automatic Match:** exactly one search result with the folder's
//!   year and the same name, ignoring case and punctuation. Its details and
//!   poster are fetched and stored ([`Catalogue::movie_matched`]).
//! - Otherwise **Needs review**, with the top [`CANDIDATES`] results kept.
//! - **TMDB unavailable** (down, rate limiting): retried with backoff
//!   ([`RETRY_DELAYS`], about an hour in all), then Needs review. The Movie
//!   keeps its folder's name and year meanwhile.
//!
//! A pass ([`Matcher::run_once`]) handles every Movie that is due; `main`
//! runs one every few seconds, tests call it explicitly. Shows get the same
//! treatment later, through the same TMDB port.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use tracing::{info, warn};
use unicode_normalization::UnicodeNormalization;

use crate::catalogue::{Candidate, Catalogue, MovieMatch, PendingLookup};
use crate::ports::tmdb::{MovieSummary, Tmdb, TmdbError};

/// How many search results a Movie in Needs review keeps.
pub const CANDIDATES: usize = 5;

/// The wait after each failed lookup in a row; after the last one the Movie
/// goes to Needs review. 30 s doubling up to 32 min: about an hour in all.
pub const RETRY_DELAYS: [Duration; 7] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(2 * 60),
    Duration::from_secs(4 * 60),
    Duration::from_secs(8 * 60),
    Duration::from_secs(16 * 60),
    Duration::from_secs(32 * 60),
];

#[derive(Clone)]
pub struct Matcher {
    catalogue: Catalogue,
    tmdb: Arc<dyn Tmdb>,
    assets: Arc<dyn ObjectStore>,
}

/// What one lookup came to.
enum Outcome {
    Matched(MovieMatch),
    NeedsReview(Vec<Candidate>),
}

/// Why a lookup did not come to an [`Outcome`].
enum LookupError {
    /// Worth trying again later (TMDB or the `assets` bucket unavailable).
    Retry(String),
    /// Trying again won't help: an admin has to look.
    Review(String),
}

impl From<TmdbError> for LookupError {
    fn from(e: TmdbError) -> Self {
        match e {
            TmdbError::Unavailable(_) => LookupError::Retry(e.to_string()),
            TmdbError::Rejected(_) => LookupError::Review(e.to_string()),
        }
    }
}

impl Matcher {
    pub fn new(catalogue: Catalogue, tmdb: Arc<dyn Tmdb>, assets: Arc<dyn ObjectStore>) -> Self {
        Matcher {
            catalogue,
            tmdb,
            assets,
        }
    }

    /// Looks up every Movie that is new or whose retry is due.
    pub async fn run_once(&self) -> Result<()> {
        for lookup in self.catalogue.pending_lookups().await? {
            self.look_up(lookup).await?;
        }
        Ok(())
    }

    async fn look_up(&self, lookup: PendingLookup) -> Result<()> {
        let movie = lookup.movie_id;
        match self.find(&lookup).await {
            Ok(Outcome::Matched(found)) => {
                info!(movie, tmdb_id = found.tmdb_id, name = %found.name, "matched");
                self.catalogue.movie_matched(movie, found).await
            }
            Ok(Outcome::NeedsReview(candidates)) => {
                info!(movie, name = %lookup.name, candidates = candidates.len(), "needs review");
                self.catalogue.movie_needs_review(movie, candidates).await
            }
            Err(LookupError::Retry(e)) => {
                let failed = lookup.failed_attempts + 1;
                match RETRY_DELAYS.get(failed as usize - 1) {
                    Some(&retry_in) => {
                        warn!(movie, error = %e, retry_in_s = retry_in.as_secs(), "lookup failed, retrying");
                        self.catalogue
                            .movie_lookup_failed(movie, failed, retry_in)
                            .await
                    }
                    None => {
                        warn!(movie, error = %e, "lookup failed too often, needs review");
                        self.catalogue.movie_needs_review(movie, Vec::new()).await
                    }
                }
            }
            Err(LookupError::Review(e)) => {
                warn!(movie, error = %e, "lookup failed, needs review");
                self.catalogue.movie_needs_review(movie, Vec::new()).await
            }
        }
    }

    async fn find(&self, lookup: &PendingLookup) -> Result<Outcome, LookupError> {
        let results = self.tmdb.search_movie(&lookup.name, lookup.year).await?;
        let mut exact = results
            .iter()
            .filter(|r| r.year == Some(lookup.year) && same_name(&r.title, &lookup.name));
        let (Some(only), None) = (exact.next(), exact.next()) else {
            return Ok(Outcome::NeedsReview(
                results
                    .into_iter()
                    .take(CANDIDATES)
                    .map(candidate)
                    .collect(),
            ));
        };
        let details = self.tmdb.movie(only.tmdb_id).await?;
        let image_key = match &details.poster_path {
            Some(path) => self.download(path).await?,
            None => None,
        };
        Ok(Outcome::Matched(MovieMatch {
            tmdb_id: details.tmdb_id,
            name: details.title,
            year: details.year.unwrap_or(lookup.year),
            imdb_id: details.imdb_id,
            description: details.overview,
            image_key,
        }))
    }

    /// Downloads the image at TMDB `path` (original size) into `assets`;
    /// returns its key there, or `None` when TMDB has no image to give (the
    /// Movie is matched without one rather than held back for it).
    async fn download(&self, path: &str) -> Result<Option<String>, LookupError> {
        let Some(key) = image_key(path) else {
            warn!(path, "TMDB image path is not a plain file name, skipped");
            return Ok(None);
        };
        let bytes = match self.tmdb.image(path).await {
            Ok(bytes) => bytes,
            Err(TmdbError::Rejected(e)) => {
                warn!(path, error = %e, "poster not available, skipped");
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let location = Path::parse(&key).expect("image keys are plain file names");
        self.assets
            .put(&location, PutPayload::from(bytes))
            .await
            .map_err(|e| LookupError::Retry(format!("cannot store {key}: {e}")))?;
        Ok(Some(key))
    }
}

fn candidate(r: MovieSummary) -> Candidate {
    Candidate {
        tmdb_id: r.tmdb_id,
        name: r.title,
        year: r.year,
        description: r.overview,
        poster_path: r.poster_path,
    }
}

/// `/abc.jpg` → `posters/abc.jpg`: TMDB's file names are unique.
fn image_key(path: &str) -> Option<String> {
    let file = path.strip_prefix('/')?;
    let ok = !file.is_empty()
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !file.starts_with('.');
    ok.then(|| format!("posters/{file}"))
}

/// The same name, ignoring case and punctuation: "Spider-Man" = "spider man".
pub fn same_name(a: &str, b: &str) -> bool {
    comparable(a) == comparable(b)
}

/// Lower case, letters and digits only, single spaces between words.
fn comparable(name: &str) -> String {
    let spaced: String = name
        .nfc()
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_without_case_and_punctuation() {
        assert!(same_name("Spider-Man", "Spider Man"));
        assert!(same_name("WALL·E", "wall e"));
        assert!(same_name("Amélie", "AMÉLIE"));
        assert!(!same_name("The Matrix", "The Matrix Reloaded"));
    }

    #[test]
    fn image_keys_are_plain_file_names() {
        assert_eq!(image_key("/abc.jpg").as_deref(), Some("posters/abc.jpg"));
        assert_eq!(image_key("/../x.jpg"), None);
        assert_eq!(image_key("abc.jpg"), None);
    }
}
