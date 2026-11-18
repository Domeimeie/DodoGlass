//! What an admin changes in the catalogue: which Media file is the Default
//! media file, Labels, removing Media files, and deleting Hidden entries.

use anyhow::Result;
use diesel::prelude::*;

use super::Catalogue;
use super::media_files::{self, READY};
use super::schema::{episode, media_file, movie, season, show};
use crate::ports::storage::delete_output;

/// The outcome of an admin change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Done,
    /// No such Media file or entry.
    NotFound,
    /// The change breaks a rule of the catalogue; the message says which.
    Refused(String),
    /// The request itself is wrong (e.g. an empty Label).
    Invalid(String),
}

/// A Movie, Show, Season or Episode, by the ids and numbers the read API uses.
#[derive(Debug, Clone, Copy)]
pub enum Entry {
    Movie(i32),
    Show(i32),
    Season { show: i32, number: i32 },
    Episode { show: i32, season: i32, number: i32 },
}

fn refused(message: &str) -> Result<Change> {
    Ok(Change::Refused(message.into()))
}

/// The rows an entry is made of: itself, what is below it, and the Media
/// files of all of them.
#[derive(Default)]
struct Scope {
    movie: Option<i32>,
    show: Option<i32>,
    seasons: Vec<i32>,
    episodes: Vec<i32>,
}

/// The rows of `entry`; `None` if it does not exist.
fn scope_of(conn: &mut SqliteConnection, entry: Entry) -> Result<Option<Scope>> {
    let episodes_of = |conn: &mut SqliteConnection, seasons: &[i32]| -> Result<Vec<i32>> {
        Ok(episode::table
            .filter(episode::season_id.eq_any(seasons))
            .select(episode::id)
            .load(conn)?)
    };
    let season_of = |conn: &mut SqliteConnection, show: i32, number: i32| -> Result<Option<i32>> {
        Ok(season::table
            .filter(season::show_id.eq(show))
            .filter(season::number.eq(number))
            .select(season::id)
            .first(conn)
            .optional()?)
    };
    Ok(match entry {
        Entry::Movie(id) => {
            let found: Option<i32> = movie::table
                .find(id)
                .select(movie::id)
                .first(conn)
                .optional()?;
            found.map(|id| Scope {
                movie: Some(id),
                ..Scope::default()
            })
        }
        Entry::Show(id) => {
            let found: Option<i32> = show::table
                .find(id)
                .select(show::id)
                .first(conn)
                .optional()?;
            match found {
                None => None,
                Some(id) => {
                    let seasons: Vec<i32> = season::table
                        .filter(season::show_id.eq(id))
                        .select(season::id)
                        .load(conn)?;
                    let episodes = episodes_of(conn, &seasons)?;
                    Some(Scope {
                        show: Some(id),
                        seasons,
                        episodes,
                        ..Scope::default()
                    })
                }
            }
        }
        Entry::Season { show, number } => match season_of(conn, show, number)? {
            None => None,
            Some(id) => Some(Scope {
                episodes: episodes_of(conn, &[id])?,
                seasons: vec![id],
                ..Scope::default()
            }),
        },
        Entry::Episode {
            show,
            season,
            number,
        } => {
            let found: Option<i32> = match season_of(conn, show, season)? {
                None => None,
                Some(season) => episode::table
                    .filter(episode::season_id.eq(season))
                    .filter(episode::number.eq(number))
                    .select(episode::id)
                    .first(conn)
                    .optional()?,
            };
            found.map(|id| Scope {
                episodes: vec![id],
                ..Scope::default()
            })
        }
    })
}

impl Scope {
    /// The Media files of the entry, with their states.
    fn media_files(&self, conn: &mut SqliteConnection) -> Result<Vec<(String, String)>> {
        let select = (media_file::id, media_file::state);
        Ok(match self.movie {
            Some(movie) => media_file::table
                .filter(media_file::movie_id.eq(movie))
                .select(select)
                .load(conn)?,
            None => media_file::table
                .filter(media_file::episode_id.eq_any(&self.episodes))
                .select(select)
                .load(conn)?,
        })
    }

    fn delete(&self, conn: &mut SqliteConnection) -> Result<()> {
        if let Some(id) = self.movie {
            diesel::delete(media_file::table.filter(media_file::movie_id.eq(id))).execute(conn)?;
            // Its metadata and candidates go with it (ON DELETE CASCADE).
            diesel::delete(movie::table.find(id)).execute(conn)?;
        }
        diesel::delete(media_file::table.filter(media_file::episode_id.eq_any(&self.episodes)))
            .execute(conn)?;
        diesel::delete(episode::table.filter(episode::id.eq_any(&self.episodes))).execute(conn)?;
        diesel::delete(season::table.filter(season::id.eq_any(&self.seasons))).execute(conn)?;
        if let Some(id) = self.show {
            diesel::delete(show::table.find(id)).execute(conn)?;
        }
        Ok(())
    }
}

const VISIBLE: &str = "only a Hidden entry can be deleted: it has a Ready Media file";

impl Catalogue {
    /// Deletes Hidden `entry` for good: with what is below it (Seasons,
    /// Episodes), its metadata and its Media files with their output in
    /// `final`. An entry that is not Hidden is refused.
    pub async fn delete_hidden(&self, entry: Entry) -> Result<Change> {
        let files = self
            .db
            .run(move |conn| {
                let Some(scope) = scope_of(conn, entry)? else {
                    return Ok(None);
                };
                Ok(Some(scope.media_files(conn)?))
            })
            .await?;
        let Some(files) = files else {
            return Ok(Change::NotFound);
        };
        if files.iter().any(|(_, state)| state == READY) {
            return refused(VISIBLE);
        }
        for (id, _) in &files {
            delete_output(self.final_.as_ref(), id).await?;
        }
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    // Looked up again: something may have changed meanwhile.
                    let Some(scope) = scope_of(conn, entry)? else {
                        return Ok(Change::NotFound);
                    };
                    if scope.media_files(conn)?.iter().any(|(_, s)| s == READY) {
                        return refused(VISIBLE);
                    }
                    scope.delete(conn)?;
                    Ok(Change::Done)
                })
            })
            .await
    }

    /// Removes Media file `id`: its output in `final`, then its entry. Its
    /// Movie or Episode stays (Hidden if nothing Ready is left), and if it was
    /// the default, the oldest remaining Ready one takes over (or else the
    /// oldest remaining one).
    pub async fn remove_media_file(&self, id: &str) -> Result<Change> {
        let id = id.to_string();
        let lookup = id.clone();
        let exists = self
            .db
            .run(move |conn| Ok(media_files::owner_of(conn, &lookup)?.is_some()))
            .await?;
        if !exists {
            return Ok(Change::NotFound);
        }
        // The output first: if this is cut short, the entry is still there
        // (Missing at worst) and the admin can remove it again.
        delete_output(self.final_.as_ref(), &id).await?;
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    media_files::hand_over_default_before_removal(conn, &id)?;
                    diesel::delete(media_file::table.find(&id)).execute(conn)?;
                    Ok(Change::Done)
                })
            })
            .await
    }

    /// Renames the Label of Media file `id`. Labels stay unique within the
    /// Movie or Episode, so a Label another of its Media files has is refused.
    pub async fn rename_label(&self, id: &str, label: &str) -> Result<Change> {
        let id = id.to_string();
        let label = label.trim().to_string();
        if label.is_empty() {
            return Ok(Change::Invalid("a Label cannot be empty".into()));
        }
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let Some((owner, _)) = media_files::owner_of(conn, &id)? else {
                        return Ok(Change::NotFound);
                    };
                    if let Some(owner) = owner
                        && media_files::other_labels(conn, owner, &id)?.contains(&label)
                    {
                        return refused("another Media file of this entry has that Label");
                    }
                    diesel::update(media_file::table.find(&id))
                        .set(media_file::label.eq(&label))
                        .execute(conn)?;
                    Ok(Change::Done)
                })
            })
            .await
    }

    /// Makes Ready Media file `id` the Default media file of its Movie or
    /// Episode.
    pub async fn set_default_media_file(&self, id: &str) -> Result<Change> {
        let id = id.to_string();
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let Some((owner, _)) = media_files::owner_of(conn, &id)? else {
                        return Ok(Change::NotFound);
                    };
                    let Some(owner) = owner else {
                        return refused("an Unassigned media file has no default");
                    };
                    let state: String = media_file::table
                        .find(&id)
                        .select(media_file::state)
                        .first(conn)?;
                    if state != READY {
                        return refused("only a Ready Media file can be the default");
                    }
                    media_files::make_default(conn, owner, &id)?;
                    Ok(Change::Done)
                })
            })
            .await
    }
}
