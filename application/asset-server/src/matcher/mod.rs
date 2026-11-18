//! The matcher: looks new Movies and Shows up in TMDB, once, when they
//! first appear.
//!
//! - **Strict automatic Match:** exactly one search result with the folder's
//!   year (a Show's first-air year) and the same name, ignoring case and
//!   punctuation. Its details and poster are fetched and stored
//!   ([`Catalogue::movie_matched`], [`Catalogue::show_matched`]).
//! - Otherwise **Needs review**, with the top [`CANDIDATES`] results kept.
//! - **TMDB unavailable** (down, rate limiting): retried with backoff
//!   ([`RETRY_DELAYS`], about an hour in all), then Needs review. The entry
//!   keeps its folder's name and year meanwhile.
//! - **Seasons and Episodes** of a matched Show are looked up by number, with
//!   no search: TMDB's answer, or nothing when it does not know them (their
//!   number stays their name, they are not in Needs review). Retried with the
//!   same backoff while TMDB is unavailable. One that appears later is looked
//!   up by the next pass, without searching the Show again.
//!
//! A pass ([`Matcher::run_once`]) handles every entry that is due: Movies,
//! then Shows, then the Seasons and Episodes of matched Shows (so a Show
//! matched in a pass has its Seasons and Episodes filled in by the same
//! pass). `main` runs one every few seconds, tests call it explicitly.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use tracing::{info, warn};

mod admin;

pub use admin::Corrected;
use unicode_normalization::UnicodeNormalization;

use crate::catalogue::{
    Candidate, Catalogue, EpisodeMatch, MovieMatch, Part, PendingLookup, PendingPart,
    PendingShowLookup, SeasonMatch, ShowMatch,
};
use crate::ports::tmdb::{MovieSummary, ShowSummary, Tmdb, TmdbError};

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
enum Outcome<M> {
    Matched(M),
    NeedsReview(Vec<Candidate>),
}

/// How long to wait after `failed` lookups in a row; `None` once the
/// retries ran out.
fn retry_delay(failed: i32) -> Option<Duration> {
    RETRY_DELAYS
        .get(usize::try_from(failed).ok()?.checked_sub(1)?)
        .copied()
}

/// The one search result with exactly this name and year, or the top
/// [`CANDIDATES`] results when there is not exactly one.
fn pick_exact(results: Vec<Candidate>, name: &str, year: i32) -> Result<Candidate, Vec<Candidate>> {
    let mut exact = results
        .iter()
        .filter(|r| r.year == Some(year) && same_name(&r.name, name));
    match (exact.next(), exact.next()) {
        (Some(only), None) => Ok(only.clone()),
        _ => Err(results.into_iter().take(CANDIDATES).collect()),
    }
}

/// Why a lookup did not come to an [`Outcome`].
pub enum LookupError {
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

    /// Looks up every Movie, Show, Season and Episode that is new or whose
    /// retry is due.
    pub async fn run_once(&self) -> Result<()> {
        for lookup in self.catalogue.pending_lookups().await? {
            self.look_up(lookup).await?;
        }
        for lookup in self.catalogue.pending_show_lookups().await? {
            self.look_up_show(lookup).await?;
        }
        for pending in self.catalogue.pending_parts().await? {
            self.look_up_part(pending).await?;
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
                match retry_delay(failed) {
                    Some(retry_in) => {
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

    async fn find(&self, lookup: &PendingLookup) -> Result<Outcome<MovieMatch>, LookupError> {
        let results = self.tmdb.search_movie(&lookup.name, lookup.year).await?;
        let results = results.into_iter().map(movie_candidate).collect();
        match pick_exact(results, &lookup.name, lookup.year) {
            Ok(only) => Ok(Outcome::Matched(
                self.fetch_movie(only.tmdb_id, Some(lookup.year)).await?,
            )),
            Err(candidates) => Ok(Outcome::NeedsReview(candidates)),
        }
    }

    /// The details and poster of TMDB Movie `tmdb_id`. `fallback_year` stands
    /// in when TMDB has no release date; without either, it is refused.
    async fn fetch_movie(
        &self,
        tmdb_id: i64,
        fallback_year: Option<i32>,
    ) -> Result<MovieMatch, LookupError> {
        let details = self.tmdb.movie(tmdb_id).await?;
        let year = details
            .year
            .or(fallback_year)
            .ok_or_else(|| LookupError::Review(format!("TMDB has no year for movie {tmdb_id}")))?;
        let image_key = self.download_opt(details.poster_path.as_deref()).await?;
        Ok(MovieMatch {
            tmdb_id: details.tmdb_id,
            name: details.title,
            year,
            imdb_id: details.imdb_id,
            description: details.overview,
            image_key,
        })
    }

    async fn look_up_show(&self, lookup: PendingShowLookup) -> Result<()> {
        let show = lookup.show_id;
        match self.find_show(&lookup).await {
            Ok(Outcome::Matched(found)) => {
                info!(show, tmdb_id = found.tmdb_id, name = %found.name, "show matched");
                self.catalogue.show_matched(show, found).await
            }
            Ok(Outcome::NeedsReview(candidates)) => {
                info!(show, name = %lookup.name, candidates = candidates.len(), "show needs review");
                self.catalogue.show_needs_review(show, candidates).await
            }
            Err(LookupError::Retry(e)) => {
                let failed = lookup.failed_attempts + 1;
                match retry_delay(failed) {
                    Some(retry_in) => {
                        warn!(show, error = %e, retry_in_s = retry_in.as_secs(), "show lookup failed, retrying");
                        self.catalogue
                            .show_lookup_failed(show, failed, retry_in)
                            .await
                    }
                    None => {
                        warn!(show, error = %e, "show lookup failed too often, needs review");
                        self.catalogue.show_needs_review(show, Vec::new()).await
                    }
                }
            }
            Err(LookupError::Review(e)) => {
                warn!(show, error = %e, "show lookup failed, needs review");
                self.catalogue.show_needs_review(show, Vec::new()).await
            }
        }
    }

    async fn find_show(
        &self,
        lookup: &PendingShowLookup,
    ) -> Result<Outcome<ShowMatch>, LookupError> {
        let results = self.tmdb.search_tv(&lookup.name, lookup.year).await?;
        let results = results.into_iter().map(show_candidate).collect();
        match pick_exact(results, &lookup.name, lookup.year) {
            Ok(only) => Ok(Outcome::Matched(
                self.fetch_show(only.tmdb_id, Some(lookup.year)).await?,
            )),
            Err(candidates) => Ok(Outcome::NeedsReview(candidates)),
        }
    }

    /// The details and poster of TMDB Show `tmdb_id`, as [`Self::fetch_movie`].
    async fn fetch_show(
        &self,
        tmdb_id: i64,
        fallback_year: Option<i32>,
    ) -> Result<ShowMatch, LookupError> {
        let details = self.tmdb.tv(tmdb_id).await?;
        let year = details
            .year
            .or(fallback_year)
            .ok_or_else(|| LookupError::Review(format!("TMDB has no year for show {tmdb_id}")))?;
        let image_key = self.download_opt(details.poster_path.as_deref()).await?;
        Ok(ShowMatch {
            tmdb_id: details.tmdb_id,
            name: details.name,
            year,
            imdb_id: details.imdb_id,
            description: details.overview,
            image_key,
        })
    }

    /// Looks a Season or Episode of a matched Show up by number. TMDB not
    /// knowing it, or refusing, leaves its number as its name: it never
    /// goes to Needs review.
    async fn look_up_part(&self, pending: PendingPart) -> Result<()> {
        let part = pending.part;
        let outcome = match part {
            Part::Season { season_id } => match self.find_season(&pending).await {
                Ok(found) => return self.catalogue.season_looked_up(season_id, found).await,
                Err(e) => e,
            },
            Part::Episode { episode_id, number } => {
                match self.find_episode(&pending, number).await {
                    Ok(found) => return self.catalogue.episode_looked_up(episode_id, found).await,
                    Err(e) => e,
                }
            }
        };
        let give_up = match outcome {
            LookupError::Retry(e) => {
                let failed = pending.failed_attempts + 1;
                if let Some(retry_in) = retry_delay(failed) {
                    warn!(?part, error = %e, retry_in_s = retry_in.as_secs(), "lookup failed, retrying");
                    return self
                        .catalogue
                        .part_lookup_failed(part, failed, retry_in)
                        .await;
                }
                e
            }
            LookupError::Review(e) => e,
        };
        warn!(?part, error = %give_up, "lookup failed, keeping the number as the name");
        match part {
            Part::Season { season_id } => self.catalogue.season_looked_up(season_id, None).await,
            Part::Episode { episode_id, .. } => {
                self.catalogue.episode_looked_up(episode_id, None).await
            }
        }
    }

    async fn find_season(&self, p: &PendingPart) -> Result<Option<SeasonMatch>, LookupError> {
        let Some(season) = self.tmdb.season(p.show_tmdb_id, p.season).await? else {
            return Ok(None);
        };
        Ok(Some(SeasonMatch {
            name: season.name,
            description: season.overview,
            image_key: self.download_opt(season.poster_path.as_deref()).await?,
        }))
    }

    async fn find_episode(
        &self,
        p: &PendingPart,
        number: i32,
    ) -> Result<Option<EpisodeMatch>, LookupError> {
        let Some(episode) = self.tmdb.episode(p.show_tmdb_id, p.season, number).await? else {
            return Ok(None);
        };
        Ok(Some(EpisodeMatch {
            tmdb_id: episode.tmdb_id,
            name: episode.name,
            description: episode.overview,
            image_key: self.download_opt(episode.still_path.as_deref()).await?,
            air_date: episode.air_date,
            imdb_id: episode.imdb_id,
        }))
    }

    /// [`Self::download`], when there is an image at all.
    async fn download_opt(&self, path: Option<&str>) -> Result<Option<String>, LookupError> {
        match path {
            Some(path) => self.download(path).await,
            None => Ok(None),
        }
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

fn movie_candidate(r: MovieSummary) -> Candidate {
    Candidate {
        tmdb_id: r.tmdb_id,
        name: r.title,
        year: r.year,
        description: r.overview,
        poster_path: r.poster_path,
    }
}

fn show_candidate(r: ShowSummary) -> Candidate {
    Candidate {
        tmdb_id: r.tmdb_id,
        name: r.name,
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
    fn retries_wait_longer_each_time_then_stop() {
        assert_eq!(retry_delay(1), Some(Duration::from_secs(30)));
        assert_eq!(retry_delay(7), Some(Duration::from_secs(32 * 60)));
        assert_eq!(retry_delay(8), None);
        assert_eq!(retry_delay(0), None);
    }

    #[test]
    fn image_keys_are_plain_file_names() {
        assert_eq!(image_key("/abc.jpg").as_deref(), Some("posters/abc.jpg"));
        assert_eq!(image_key("/../x.jpg"), None);
        assert_eq!(image_key("abc.jpg"), None);
    }
}
