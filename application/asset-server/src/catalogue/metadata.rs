//! Movie metadata: what a TMDB lookup found (a Match, Needs review with its
//! candidates, or a retry while TMDB is unavailable), and Locked fields.
//!
//! The lookups themselves are the matcher's job (`crate::matcher`); this module
//! only stores their outcome. A Movie without a `movie_metadata` row has not
//! been looked up yet; one in `retrying` is looked up again from `retry_at_ms`.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Result;
use diesel::prelude::*;
use serde::Serialize;

use super::schema::{movie, movie_candidate, movie_metadata};
use super::{Catalogue, MovieView};

const MATCHED: &str = "matched";
const NEEDS_REVIEW: &str = "needs_review";
const RETRYING: &str = "retrying";

/// The descriptive fields an admin can lock (as stored in `locked_fields`).
pub const LOCKABLE_FIELDS: [&str; 5] = ["name", "year", "imdb_id", "description", "image"];

/// The Locked fields of an entry: a JSON array of field names in the database.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LockedFields(BTreeSet<String>);

impl LockedFields {
    pub fn parse(json: &str) -> LockedFields {
        LockedFields(serde_json::from_str(json).unwrap_or_default())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.0).expect("a set of strings serialises")
    }

    pub fn contains(&self, field: &str) -> bool {
        self.0.contains(field)
    }

    pub fn lock(&mut self, field: &str) {
        self.0.insert(field.into());
    }

    pub fn unlock(&mut self, field: &str) {
        self.0.remove(field);
    }
}

/// A Movie waiting for a TMDB lookup.
#[derive(Debug, Clone)]
pub struct PendingLookup {
    pub movie_id: i32,
    /// Name and year from the upload folder: what TMDB is searched for.
    pub name: String,
    pub year: i32,
    /// Lookups that already failed because TMDB was unavailable.
    pub failed_attempts: i32,
}

/// What a Match fills in.
#[derive(Debug, Clone)]
pub struct MovieMatch {
    pub tmdb_id: i64,
    pub name: String,
    pub year: i32,
    pub imdb_id: Option<String>,
    pub description: Option<String>,
    /// Key of the downloaded poster in `assets`.
    pub image_key: Option<String>,
}

/// One of the TMDB results kept for an entry in Needs review.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub tmdb_id: i64,
    pub name: String,
    pub year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// TMDB's image path (`/abc.jpg`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster_path: Option<String>,
}

/// The metadata part of a Movie in the read API; absent until a Match.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MovieMetadataView {
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

/// A Movie in Needs review, as the admin API lists it.
#[derive(Debug, Clone, Serialize)]
pub struct NeedsReviewMovie {
    pub id: i32,
    pub name: String,
    pub year: i32,
    pub candidates: Vec<Candidate>,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = movie_metadata)]
struct MetadataRow {
    movie_id: i32,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
    description: Option<String>,
    image_key: Option<String>,
}

/// Fills the metadata part of `movies` (keyed by Movie id).
pub(super) fn fill_views(
    conn: &mut SqliteConnection,
    movies: &mut [(i32, MovieView)],
) -> Result<()> {
    let ids: Vec<i32> = movies.iter().map(|(id, _)| *id).collect();
    let rows: Vec<MetadataRow> = movie_metadata::table
        .filter(movie_metadata::movie_id.eq_any(&ids))
        .filter(movie_metadata::match_state.eq(MATCHED))
        .select(MetadataRow::as_select())
        .load(conn)?;
    for row in rows {
        if let Some((_, view)) = movies.iter_mut().find(|(id, _)| *id == row.movie_id) {
            view.metadata = MovieMetadataView {
                tmdb_id: row.tmdb_id,
                imdb_id: row.imdb_id,
                description: row.description,
                image: row.image_key,
            };
        }
    }
    Ok(())
}

fn locked_fields(conn: &mut SqliteConnection, movie_id: i32) -> Result<LockedFields> {
    let json: Option<String> = movie_metadata::table
        .find(movie_id)
        .select(movie_metadata::locked_fields)
        .first(conn)
        .optional()?;
    Ok(json.map(|j| LockedFields::parse(&j)).unwrap_or_default())
}

impl Catalogue {
    /// The Movies to look up now: new ones, and those whose retry is due.
    pub async fn pending_lookups(&self) -> Result<Vec<PendingLookup>> {
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                let rows: Vec<(i32, String, i32, Option<i32>)> = movie::table
                    .left_join(movie_metadata::table)
                    .filter(
                        movie_metadata::movie_id
                            .is_null()
                            .or(movie_metadata::match_state
                                .eq(RETRYING)
                                .and(movie_metadata::retry_at_ms.le(now))),
                    )
                    .order(movie::id.asc())
                    .select((
                        movie::id,
                        movie::folder_name,
                        movie::folder_year,
                        movie_metadata::lookup_attempts.nullable(),
                    ))
                    .load(conn)?;
                Ok(rows
                    .into_iter()
                    .map(|(movie_id, name, year, attempts)| PendingLookup {
                        movie_id,
                        name,
                        year,
                        failed_attempts: attempts.unwrap_or(0),
                    })
                    .collect())
            })
            .await
    }

    /// The Movie has a Match: every field that is not locked is filled in.
    pub async fn movie_matched(&self, movie_id: i32, found: MovieMatch) -> Result<()> {
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let locks = locked_fields(conn, movie_id)?;
                    let current: Option<MetadataRow> = movie_metadata::table
                        .find(movie_id)
                        .select(MetadataRow::as_select())
                        .first(conn)
                        .optional()?;
                    let keep = |field: &str, new: Option<String>, old: Option<String>| {
                        if locks.contains(field) { old } else { new }
                    };
                    let (imdb_id, description, image_key) = match current {
                        Some(c) => (
                            keep("imdb_id", found.imdb_id, c.imdb_id),
                            keep("description", found.description, c.description),
                            keep("image", found.image_key, c.image_key),
                        ),
                        None => (found.imdb_id, found.description, found.image_key),
                    };
                    if !locks.contains("name") {
                        diesel::update(movie::table.find(movie_id))
                            .set(movie::name.eq(&found.name))
                            .execute(conn)?;
                    }
                    if !locks.contains("year") {
                        diesel::update(movie::table.find(movie_id))
                            .set(movie::year.eq(found.year))
                            .execute(conn)?;
                    }
                    let values = (
                        movie_metadata::match_state.eq(MATCHED),
                        movie_metadata::lookup_attempts.eq(0),
                        movie_metadata::retry_at_ms.eq(None::<i64>),
                        movie_metadata::tmdb_id.eq(Some(found.tmdb_id)),
                        movie_metadata::imdb_id.eq(imdb_id),
                        movie_metadata::description.eq(description),
                        movie_metadata::image_key.eq(image_key),
                    );
                    diesel::insert_into(movie_metadata::table)
                        .values((movie_metadata::movie_id.eq(movie_id), values.clone()))
                        .on_conflict(movie_metadata::movie_id)
                        .do_update()
                        .set(values)
                        .execute(conn)?;
                    diesel::delete(
                        movie_candidate::table.filter(movie_candidate::movie_id.eq(movie_id)),
                    )
                    .execute(conn)?;
                    Ok(())
                })
            })
            .await
    }

    /// No certain Match: the Movie is in Needs review, with these candidates
    /// (best first). It keeps the name and year from its folder.
    pub async fn movie_needs_review(
        &self,
        movie_id: i32,
        candidates: Vec<Candidate>,
    ) -> Result<()> {
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let values = (
                        movie_metadata::match_state.eq(NEEDS_REVIEW),
                        movie_metadata::lookup_attempts.eq(0),
                        movie_metadata::retry_at_ms.eq(None::<i64>),
                    );
                    diesel::insert_into(movie_metadata::table)
                        .values((movie_metadata::movie_id.eq(movie_id), values))
                        .on_conflict(movie_metadata::movie_id)
                        .do_update()
                        .set(values)
                        .execute(conn)?;
                    diesel::delete(
                        movie_candidate::table.filter(movie_candidate::movie_id.eq(movie_id)),
                    )
                    .execute(conn)?;
                    let rows: Vec<_> = candidates
                        .into_iter()
                        .enumerate()
                        .map(|(i, c)| {
                            (
                                movie_candidate::movie_id.eq(movie_id),
                                movie_candidate::position.eq(i as i32),
                                movie_candidate::tmdb_id.eq(c.tmdb_id),
                                movie_candidate::name.eq(c.name),
                                movie_candidate::year.eq(c.year),
                                movie_candidate::description.eq(c.description),
                                movie_candidate::poster_path.eq(c.poster_path),
                            )
                        })
                        .collect();
                    diesel::insert_into(movie_candidate::table)
                        .values(rows)
                        .execute(conn)?;
                    Ok(())
                })
            })
            .await
    }

    /// TMDB was unavailable: look the Movie up again in `retry_in`. It keeps
    /// the name and year from its folder meanwhile.
    pub async fn movie_lookup_failed(
        &self,
        movie_id: i32,
        failed_attempts: i32,
        retry_in: Duration,
    ) -> Result<()> {
        let retry_at_ms = self.clock.now_ms() + retry_in.as_millis() as i64;
        self.db
            .run(move |conn| {
                let values = (
                    movie_metadata::match_state.eq(RETRYING),
                    movie_metadata::lookup_attempts.eq(failed_attempts),
                    movie_metadata::retry_at_ms.eq(Some(retry_at_ms)),
                );
                diesel::insert_into(movie_metadata::table)
                    .values((movie_metadata::movie_id.eq(movie_id), values))
                    .on_conflict(movie_metadata::movie_id)
                    .do_update()
                    .set(values)
                    .execute(conn)?;
                Ok(())
            })
            .await
    }

    /// The Movies in Needs review, by name, each with its candidates.
    pub async fn movies_needing_review(&self) -> Result<Vec<NeedsReviewMovie>> {
        self.db
            .run(|conn| {
                let movies: Vec<(i32, String, i32)> = movie::table
                    .inner_join(movie_metadata::table)
                    .filter(movie_metadata::match_state.eq(NEEDS_REVIEW))
                    .order((movie::name.asc(), movie::year.asc(), movie::id.asc()))
                    .select((movie::id, movie::name, movie::year))
                    .load(conn)?;
                let ids: Vec<i32> = movies.iter().map(|(id, _, _)| *id).collect();
                #[allow(clippy::type_complexity)]
                let candidates: Vec<(
                    i32,
                    i64,
                    String,
                    Option<i32>,
                    Option<String>,
                    Option<String>,
                )> = movie_candidate::table
                    .filter(movie_candidate::movie_id.eq_any(&ids))
                    .order((movie_candidate::movie_id, movie_candidate::position))
                    .select((
                        movie_candidate::movie_id,
                        movie_candidate::tmdb_id,
                        movie_candidate::name,
                        movie_candidate::year,
                        movie_candidate::description,
                        movie_candidate::poster_path,
                    ))
                    .load(conn)?;
                Ok(movies
                    .into_iter()
                    .map(|(id, name, year)| NeedsReviewMovie {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_fields_round_trip_as_a_json_array() {
        let mut locks = LockedFields::parse("[]");
        locks.lock("description");
        locks.lock("name");
        assert_eq!(locks.to_json(), r#"["description","name"]"#);
        assert!(LockedFields::parse(&locks.to_json()).contains("name"));
        locks.unlock("name");
        assert!(!locks.contains("name"));
    }
}
