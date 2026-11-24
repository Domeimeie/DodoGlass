//! Lookups an admin asks for: picking a Match (one of the candidates or any
//! TMDB id), refreshing an entry, and attaching an Unassigned media file to a
//! new Movie or Show created from a TMDB id. Locked fields are kept, as in
//! every lookup.

use anyhow::Result;
use serde::Deserialize;

use super::{LookupError, Matcher, Seasons};
use crate::catalogue::{Attached, Entry, PartRefresh};

/// A new Movie or Episode an admin attaches an Unassigned media file to,
/// created from a TMDB id.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachNew {
    NewMovie {
        tmdb_id: i64,
    },
    /// Episode `number` of Season `season` of the TMDB Show `tmdb_id`.
    NewEpisode {
        tmdb_id: i64,
        season: u32,
        number: u32,
    },
}

/// What came of an admin's lookup: done, or why not.
pub type Corrected = std::result::Result<(), CorrectionFailed>;

/// Why an admin's lookup was not done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorrectionFailed {
    /// No such entry (or Unassigned media file); the message says which.
    NotFound(String),
    /// The entry cannot be looked up as asked; the message says why.
    Refused(String),
    /// TMDB refused: no such id, or nothing usable under it.
    Rejected(String),
    /// TMDB (or the `assets` bucket) is unavailable: try again later.
    Unavailable(String),
}

impl From<LookupError> for CorrectionFailed {
    fn from(e: LookupError) -> Self {
        match e {
            LookupError::Retry(e) => CorrectionFailed::Unavailable(e),
            LookupError::Review(e) => CorrectionFailed::Rejected(e),
        }
    }
}

impl Matcher {
    /// Makes TMDB Movie `tmdb_id` the Match of Movie `movie_id`: every field
    /// that is not locked is refilled, and it leaves Needs review.
    pub async fn pick_movie_match(&self, movie_id: i32, tmdb_id: i64) -> Result<Corrected> {
        let Some(movie) = self.catalogue.movie_lookup(movie_id).await? else {
            return Ok(Err(CorrectionFailed::NotFound("no such movie".into())));
        };
        let found = match self.fetch_movie(tmdb_id, Some(movie.lookup.year)).await {
            Ok(found) => found,
            Err(e) => return Ok(Err(e.into())),
        };
        self.catalogue.movie_matched(movie_id, found).await?;
        Ok(Ok(()))
    }

    /// Fetches Movie `movie_id` from TMDB again: by its Match when it has
    /// one, else by searching for it as when it was new (it may then get a
    /// Match, stay in Needs review or be retried). Locked fields are kept.
    pub async fn refresh_movie(&self, movie_id: i32) -> Result<Corrected> {
        let Some(movie) = self.catalogue.movie_lookup(movie_id).await? else {
            return Ok(Err(CorrectionFailed::NotFound("no such movie".into())));
        };
        match movie.tmdb_id {
            Some(tmdb_id) => self.pick_movie_match(movie_id, tmdb_id).await,
            None => {
                self.look_up(movie.lookup).await?;
                Ok(Ok(()))
            }
        }
    }

    /// Makes TMDB Show `tmdb_id` the Match of Show `show_id`, as for a
    /// Movie; its Seasons and Episodes are then looked up again by number.
    pub async fn pick_show_match(&self, show_id: i32, tmdb_id: i64) -> Result<Corrected> {
        let Some(show) = self.catalogue.show_lookup(show_id).await? else {
            return Ok(Err(CorrectionFailed::NotFound("no such show".into())));
        };
        let found = match self.fetch_show(tmdb_id, Some(show.lookup.year)).await {
            Ok(found) => found,
            Err(e) => return Ok(Err(e.into())),
        };
        self.catalogue.show_matched(show_id, found).await?;
        self.look_up_parts_of(show_id).await?;
        Ok(Ok(()))
    }

    /// Fetches Show `show_id` from TMDB again, as for a Movie, with its
    /// Seasons and Episodes.
    pub async fn refresh_show(&self, show_id: i32) -> Result<Corrected> {
        let Some(show) = self.catalogue.show_lookup(show_id).await? else {
            return Ok(Err(CorrectionFailed::NotFound("no such show".into())));
        };
        match show.tmdb_id {
            Some(tmdb_id) => self.pick_show_match(show_id, tmdb_id).await,
            None => {
                self.look_up_show(show.lookup).await?;
                self.look_up_parts_of(show_id).await?;
                Ok(Ok(()))
            }
        }
    }

    /// Attaches the Unassigned media file `id` to a new Movie, or an Episode
    /// of a new Show, created from TMDB (with that as its Match). The Show's
    /// Seasons and Episodes are looked up by number at once. `Err` when TMDB
    /// refuses or is unavailable: nothing is created or attached then.
    pub async fn attach_new(
        &self,
        id: &str,
        to: AttachNew,
    ) -> Result<Result<Attached, CorrectionFailed>> {
        if !self.catalogue.is_unassigned_media_file(id).await? {
            return Ok(Ok(Attached::NoSuchMediaFile));
        }
        match to {
            AttachNew::NewMovie { tmdb_id } => {
                let found = match self.fetch_movie(tmdb_id, None).await {
                    Ok(found) => found,
                    Err(e) => return Ok(Err(e.into())),
                };
                Ok(Ok(self.catalogue.attach_to_new_movie(id, found).await?))
            }
            AttachNew::NewEpisode {
                tmdb_id,
                season,
                number,
            } => {
                let found = match self.fetch_show(tmdb_id, None).await {
                    Ok(found) => found,
                    Err(e) => return Ok(Err(e.into())),
                };
                let (attached, show_id) = self
                    .catalogue
                    .attach_to_new_episode(id, found, season, number)
                    .await?;
                if let Some(show_id) = show_id {
                    self.look_up_parts_of(show_id).await?;
                }
                Ok(Ok(attached))
            }
        }
    }

    /// Looks Season or Episode `entry` up again by number, now. Its Show
    /// must have a Match.
    pub async fn refresh_part(&self, entry: Entry) -> Result<Corrected> {
        match self.catalogue.look_up_part_again(entry).await? {
            PartRefresh::Due { show_id } => {
                self.look_up_parts_of(show_id).await?;
                Ok(Ok(()))
            }
            PartRefresh::NotFound => Ok(Err(CorrectionFailed::NotFound("no such entry".into()))),
            PartRefresh::ShowNotMatched => Ok(Err(CorrectionFailed::Refused(
                "its Show has no Match: pick one first".into(),
            ))),
        }
    }

    /// Looks up the Seasons and Episodes of Show `show_id` that are due now,
    /// rather than waiting for the next pass. TMDB failing for one of them
    /// leaves it to be retried, as in a pass.
    async fn look_up_parts_of(&self, show_id: i32) -> Result<()> {
        let mut seasons = Seasons::default();
        for due in self.catalogue.part_lookups_due().await? {
            if due.show_id == show_id {
                self.look_up_part(due, &mut seasons).await?;
            }
        }
        Ok(())
    }
}
