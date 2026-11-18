//! What an admin corrects in the metadata: the entries a picked Match or a
//! refresh applies to, fields edited by hand (which locks them), and
//! unlocking them. The TMDB side of picking and refreshing is the matcher's
//! (`crate::matcher`).

use anyhow::Result;
use diesel::prelude::*;

use super::Catalogue;
use super::metadata::PendingLookup;
use super::schema::{movie, movie_metadata};

const MATCHED: &str = "matched";

/// A Movie as a refresh or a picked Match needs it: what to search TMDB for,
/// and its Match, if it has one.
#[derive(Debug, Clone)]
pub struct MovieLookup {
    pub lookup: PendingLookup,
    pub tmdb_id: Option<i64>,
}

impl Catalogue {
    /// Movie `movie_id` for a refresh or a picked Match; `None` if unknown.
    pub async fn movie_lookup(&self, movie_id: i32) -> Result<Option<MovieLookup>> {
        self.db
            .run(move |conn| {
                let row: Option<(String, i32, Option<String>, Option<i64>)> = movie::table
                    .left_join(movie_metadata::table)
                    .filter(movie::id.eq(movie_id))
                    .select((
                        movie::folder_name,
                        movie::folder_year,
                        movie_metadata::match_state.nullable(),
                        movie_metadata::tmdb_id.nullable(),
                    ))
                    .first(conn)
                    .optional()?;
                Ok(row.map(|(name, year, state, tmdb_id)| MovieLookup {
                    lookup: PendingLookup {
                        movie_id,
                        name,
                        year,
                        failed_attempts: 0,
                    },
                    tmdb_id: tmdb_id.filter(|_| state.as_deref() == Some(MATCHED)),
                }))
            })
            .await
    }
}
