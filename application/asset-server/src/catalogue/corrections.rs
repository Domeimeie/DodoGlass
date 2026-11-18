//! What an admin corrects in the metadata: fields edited by hand, which
//! become Locked fields, unlocking them, and the entries a picked Match or a
//! refresh applies to. The TMDB side of picking and refreshing is the
//! matcher's (`crate::matcher`).
//!
//! An edit of an entry that was never looked up gives it a metadata row that
//! is due for a lookup (`retrying`, due at once), so that the lookup still
//! happens and fills every field that is not locked.

use anyhow::Result;
use diesel::prelude::*;
use serde_json::{Map, Value};

use super::Catalogue;
use super::admin::Entry;
use super::metadata::{LOCKABLE_FIELDS, LockedFields, LookupDue};
use super::schema::{
    episode, episode_metadata, movie, movie_metadata, season, season_metadata, show, show_metadata,
};
use super::show_metadata::{EPISODE_LOCKABLE_FIELDS, SEASON_LOCKABLE_FIELDS, SHOW_LOCKABLE_FIELDS};

const MATCHED: &str = "matched";
const RETRYING: &str = "retrying";
/// A `retry_at_ms` that is always due: looked up by the next pass.
const DUE: Option<i64> = Some(0);

/// A Movie as a refresh or a picked Match needs it: what to search TMDB for,
/// and its Match, if it has one.
#[derive(Debug, Clone)]
pub struct MovieLookup {
    pub lookup: LookupDue,
    pub tmdb_id: Option<i64>,
}

/// A Show as a refresh or a picked Match needs it, as [`MovieLookup`].
#[derive(Debug, Clone)]
pub struct ShowLookup {
    pub lookup: LookupDue,
    pub tmdb_id: Option<i64>,
}

/// What came of asking for a Season or Episode to be looked up again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartRefresh {
    /// It is due for a lookup by number, as part of this Show.
    Due {
        show_id: i32,
    },
    NotFound,
    /// Its Show has no Match, so there is nothing to look it up in.
    ShowNotMatched,
}

/// What came of editing or unlocking fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edited {
    /// Done; these are the entry's Locked fields now.
    Locked(Vec<String>),
    NotFound,
    /// Not a lockable field of this entry, or not a value it can take.
    Invalid(String),
}

/// An entry resolved to its row id.
#[derive(Debug, Clone, Copy)]
enum Target {
    Movie(i32),
    Show(i32),
    Season(i32),
    Episode(i32),
}

impl Target {
    fn lockable(self) -> &'static [&'static str] {
        match self {
            Target::Movie(_) => &LOCKABLE_FIELDS,
            Target::Show(_) => &SHOW_LOCKABLE_FIELDS,
            Target::Season(_) => &SEASON_LOCKABLE_FIELDS,
            Target::Episode(_) => &EPISODE_LOCKABLE_FIELDS,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Target::Movie(_) => "a Movie",
            Target::Show(_) => "a Show",
            Target::Season(_) => "a Season",
            Target::Episode(_) => "an Episode",
        }
    }
}

/// The id of Season `number` of Show `show`.
fn season_id(conn: &mut SqliteConnection, show: i32, number: i32) -> Result<Option<i32>> {
    Ok(season::table
        .filter(season::show_id.eq(show))
        .filter(season::number.eq(number))
        .select(season::id)
        .first(conn)
        .optional()?)
}

fn resolve(conn: &mut SqliteConnection, entry: Entry) -> Result<Option<Target>> {
    Ok(match entry {
        Entry::Movie(id) => movie::table
            .find(id)
            .select(movie::id)
            .first(conn)
            .optional()?
            .map(Target::Movie),
        Entry::Show(id) => show::table
            .find(id)
            .select(show::id)
            .first(conn)
            .optional()?
            .map(Target::Show),
        Entry::Season { show, number } => season_id(conn, show, number)?.map(Target::Season),
        Entry::Episode {
            show,
            season,
            number,
        } => match season_id(conn, show, season)? {
            None => None,
            Some(season) => episode::table
                .filter(episode::season_id.eq(season))
                .filter(episode::number.eq(number))
                .select(episode::id)
                .first(conn)
                .optional()?
                .map(Target::Episode),
        },
    })
}

/// A value an admin gives a field.
enum FieldValue {
    Text(Option<String>),
    Year(i32),
}

/// The edits in `fields`, checked against what `target` has.
fn parse_edits(
    target: Target,
    fields: Map<String, Value>,
) -> Result<Vec<(String, FieldValue)>, String> {
    if fields.is_empty() {
        return Err("no field to edit".into());
    }
    let named = matches!(target, Target::Movie(_) | Target::Show(_));
    fields
        .into_iter()
        .map(|(field, value)| {
            if !target.lockable().contains(&field.as_str()) {
                return Err(format!("{} has no field {field:?}", target.what()));
            }
            let parsed = match (field.as_str(), value) {
                ("year", Value::Number(n)) => n
                    .as_i64()
                    .and_then(|y| i32::try_from(y).ok())
                    .map(FieldValue::Year),
                ("year", _) => None,
                ("name", Value::String(s)) if s.trim().is_empty() => None,
                ("name", Value::Null) if named => None,
                (_, Value::String(s)) => Some(FieldValue::Text(Some(s))),
                (_, Value::Null) => Some(FieldValue::Text(None)),
                _ => None,
            };
            parsed
                .map(|v| (field.clone(), v))
                .ok_or_else(|| format!("not a value for {field:?}"))
        })
        .collect()
}

fn text(value: FieldValue) -> Option<String> {
    match value {
        FieldValue::Text(t) => t,
        FieldValue::Year(y) => Some(y.to_string()),
    }
}

/// Gives `target` a metadata row if it has none: due for a lookup at once.
fn ensure_metadata(conn: &mut SqliteConnection, target: Target) -> Result<()> {
    match target {
        Target::Movie(id) => diesel::insert_into(movie_metadata::table)
            .values((
                movie_metadata::movie_id.eq(id),
                movie_metadata::match_state.eq(RETRYING),
                movie_metadata::retry_at_ms.eq(DUE),
            ))
            .on_conflict_do_nothing()
            .execute(conn)?,
        Target::Show(id) => diesel::insert_into(show_metadata::table)
            .values((
                show_metadata::show_id.eq(id),
                show_metadata::match_state.eq(RETRYING),
                show_metadata::retry_at_ms.eq(DUE),
            ))
            .on_conflict_do_nothing()
            .execute(conn)?,
        Target::Season(id) => diesel::insert_into(season_metadata::table)
            .values((
                season_metadata::season_id.eq(id),
                season_metadata::match_state.eq(RETRYING),
                season_metadata::retry_at_ms.eq(DUE),
            ))
            .on_conflict_do_nothing()
            .execute(conn)?,
        Target::Episode(id) => diesel::insert_into(episode_metadata::table)
            .values((
                episode_metadata::episode_id.eq(id),
                episode_metadata::match_state.eq(RETRYING),
                episode_metadata::retry_at_ms.eq(DUE),
            ))
            .on_conflict_do_nothing()
            .execute(conn)?,
    };
    Ok(())
}

fn locks_of(conn: &mut SqliteConnection, target: Target) -> Result<LockedFields> {
    let json: String = match target {
        Target::Movie(id) => movie_metadata::table
            .find(id)
            .select(movie_metadata::locked_fields)
            .first(conn)?,
        Target::Show(id) => show_metadata::table
            .find(id)
            .select(show_metadata::locked_fields)
            .first(conn)?,
        Target::Season(id) => season_metadata::table
            .find(id)
            .select(season_metadata::locked_fields)
            .first(conn)?,
        Target::Episode(id) => episode_metadata::table
            .find(id)
            .select(episode_metadata::locked_fields)
            .first(conn)?,
    };
    Ok(LockedFields::parse(&json))
}

fn store_locks(conn: &mut SqliteConnection, target: Target, locks: &LockedFields) -> Result<()> {
    let json = locks.to_json();
    match target {
        Target::Movie(id) => diesel::update(movie_metadata::table.find(id))
            .set(movie_metadata::locked_fields.eq(json))
            .execute(conn)?,
        Target::Show(id) => diesel::update(show_metadata::table.find(id))
            .set(show_metadata::locked_fields.eq(json))
            .execute(conn)?,
        Target::Season(id) => diesel::update(season_metadata::table.find(id))
            .set(season_metadata::locked_fields.eq(json))
            .execute(conn)?,
        Target::Episode(id) => diesel::update(episode_metadata::table.find(id))
            .set(episode_metadata::locked_fields.eq(json))
            .execute(conn)?,
    };
    Ok(())
}

/// Writes `value` into `field` of `target` (already checked by [`parse_edits`]).
fn write_field(
    conn: &mut SqliteConnection,
    target: Target,
    field: &str,
    value: FieldValue,
) -> Result<()> {
    match (target, field, value) {
        (Target::Movie(id), "year", FieldValue::Year(y)) => {
            diesel::update(movie::table.find(id))
                .set(movie::year.eq(y))
                .execute(conn)?;
        }
        (Target::Show(id), "year", FieldValue::Year(y)) => {
            diesel::update(show::table.find(id))
                .set(show::year.eq(y))
                .execute(conn)?;
        }
        (target, field, value) => {
            let value = text(value);
            write_text(conn, target, field, value)?;
        }
    }
    Ok(())
}

fn write_text(
    conn: &mut SqliteConnection,
    target: Target,
    field: &str,
    value: Option<String>,
) -> Result<()> {
    use Target::*;
    let rows = match (target, field) {
        (Movie(id), "name") => diesel::update(movie::table.find(id))
            .set(movie::name.eq(value.unwrap_or_default()))
            .execute(conn)?,
        (Movie(id), "imdb_id") => diesel::update(movie_metadata::table.find(id))
            .set(movie_metadata::imdb_id.eq(value))
            .execute(conn)?,
        (Movie(id), "description") => diesel::update(movie_metadata::table.find(id))
            .set(movie_metadata::description.eq(value))
            .execute(conn)?,
        (Movie(id), "image") => diesel::update(movie_metadata::table.find(id))
            .set(movie_metadata::image_key.eq(value))
            .execute(conn)?,
        (Show(id), "name") => diesel::update(show::table.find(id))
            .set(show::name.eq(value.unwrap_or_default()))
            .execute(conn)?,
        (Show(id), "imdb_id") => diesel::update(show_metadata::table.find(id))
            .set(show_metadata::imdb_id.eq(value))
            .execute(conn)?,
        (Show(id), "description") => diesel::update(show_metadata::table.find(id))
            .set(show_metadata::description.eq(value))
            .execute(conn)?,
        (Show(id), "image") => diesel::update(show_metadata::table.find(id))
            .set(show_metadata::image_key.eq(value))
            .execute(conn)?,
        (Season(id), "name") => diesel::update(season_metadata::table.find(id))
            .set(season_metadata::name.eq(value))
            .execute(conn)?,
        (Season(id), "description") => diesel::update(season_metadata::table.find(id))
            .set(season_metadata::description.eq(value))
            .execute(conn)?,
        (Season(id), "image") => diesel::update(season_metadata::table.find(id))
            .set(season_metadata::image_key.eq(value))
            .execute(conn)?,
        (Episode(id), "name") => diesel::update(episode_metadata::table.find(id))
            .set(episode_metadata::name.eq(value))
            .execute(conn)?,
        (Episode(id), "description") => diesel::update(episode_metadata::table.find(id))
            .set(episode_metadata::description.eq(value))
            .execute(conn)?,
        (Episode(id), "image") => diesel::update(episode_metadata::table.find(id))
            .set(episode_metadata::image_key.eq(value))
            .execute(conn)?,
        (Episode(id), "air_date") => diesel::update(episode_metadata::table.find(id))
            .set(episode_metadata::air_date.eq(value))
            .execute(conn)?,
        (Episode(id), "imdb_id") => diesel::update(episode_metadata::table.find(id))
            .set(episode_metadata::imdb_id.eq(value))
            .execute(conn)?,
        (target, field) => anyhow::bail!("{} has no field {field:?}", target.what()),
    };
    anyhow::ensure!(rows == 1, "{field} of {target:?} not written");
    Ok(())
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
                    lookup: LookupDue {
                        id: movie_id,
                        name,
                        year,
                        failed_attempts: 0,
                    },
                    tmdb_id: tmdb_id.filter(|_| state.as_deref() == Some(MATCHED)),
                }))
            })
            .await
    }

    /// Show `show_id` for a refresh or a picked Match; `None` if unknown.
    pub async fn show_lookup(&self, show_id: i32) -> Result<Option<ShowLookup>> {
        self.db
            .run(move |conn| {
                let row: Option<(String, i32, Option<String>, Option<i64>)> = show::table
                    .left_join(show_metadata::table)
                    .filter(show::id.eq(show_id))
                    .select((
                        show::folder_name,
                        show::folder_year,
                        show_metadata::match_state.nullable(),
                        show_metadata::tmdb_id.nullable(),
                    ))
                    .first(conn)
                    .optional()?;
                Ok(row.map(|(name, year, state, tmdb_id)| ShowLookup {
                    lookup: LookupDue {
                        id: show_id,
                        name,
                        year,
                        failed_attempts: 0,
                    },
                    tmdb_id: tmdb_id.filter(|_| state.as_deref() == Some(MATCHED)),
                }))
            })
            .await
    }

    /// Makes Season or Episode `entry` due for a lookup by number; the
    /// lookup keeps its Locked fields.
    pub async fn look_up_part_again(&self, entry: Entry) -> Result<PartRefresh> {
        let show_id = match entry {
            Entry::Season { show, .. } | Entry::Episode { show, .. } => show,
            Entry::Movie(_) | Entry::Show(_) => {
                anyhow::bail!("{entry:?} is not a Season or Episode")
            }
        };
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let Some(target) = resolve(conn, entry)? else {
                        return Ok(PartRefresh::NotFound);
                    };
                    let state: Option<String> = show_metadata::table
                        .find(show_id)
                        .select(show_metadata::match_state)
                        .first(conn)
                        .optional()?;
                    if state.as_deref() != Some(MATCHED) {
                        return Ok(PartRefresh::ShowNotMatched);
                    }
                    ensure_metadata(conn, target)?;
                    match target {
                        Target::Season(id) => diesel::update(season_metadata::table.find(id))
                            .set((
                                season_metadata::match_state.eq(RETRYING),
                                season_metadata::lookup_attempts.eq(0),
                                season_metadata::retry_at_ms.eq(DUE),
                            ))
                            .execute(conn)?,
                        Target::Episode(id) => diesel::update(episode_metadata::table.find(id))
                            .set((
                                episode_metadata::match_state.eq(RETRYING),
                                episode_metadata::lookup_attempts.eq(0),
                                episode_metadata::retry_at_ms.eq(DUE),
                            ))
                            .execute(conn)?,
                        Target::Movie(_) | Target::Show(_) => unreachable!("resolved from a part"),
                    };
                    Ok(PartRefresh::Due { show_id })
                })
            })
            .await
    }

    /// Sets these fields of `entry` (field name → value; `null` empties a
    /// field that can be empty) and locks them: no Match, lookup or refresh
    /// overwrites them until they are unlocked.
    pub async fn edit_fields(&self, entry: Entry, fields: Map<String, Value>) -> Result<Edited> {
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let Some(target) = resolve(conn, entry)? else {
                        return Ok(Edited::NotFound);
                    };
                    let edits = match parse_edits(target, fields) {
                        Ok(edits) => edits,
                        Err(message) => return Ok(Edited::Invalid(message)),
                    };
                    ensure_metadata(conn, target)?;
                    let mut locks = locks_of(conn, target)?;
                    for (field, value) in edits {
                        write_field(conn, target, &field, value)?;
                        locks.lock(&field);
                    }
                    store_locks(conn, target, &locks)?;
                    Ok(Edited::Locked(locks.fields()))
                })
            })
            .await
    }

    /// Unlocks `field` of `entry`: the next Match or refresh fills it from
    /// TMDB again. Unlocking a field that is not locked changes nothing.
    pub async fn unlock_field(&self, entry: Entry, field: String) -> Result<Edited> {
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let Some(target) = resolve(conn, entry)? else {
                        return Ok(Edited::NotFound);
                    };
                    if !target.lockable().contains(&field.as_str()) {
                        let message = format!("{} has no field {field:?}", target.what());
                        return Ok(Edited::Invalid(message));
                    }
                    ensure_metadata(conn, target)?;
                    let mut locks = locks_of(conn, target)?;
                    locks.unlock(&field);
                    store_locks(conn, target, &locks)?;
                    Ok(Edited::Locked(locks.fields()))
                })
            })
            .await
    }
}
