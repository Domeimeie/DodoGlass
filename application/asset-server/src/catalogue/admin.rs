//! What an admin changes in the catalogue: which Media file is the Default
//! media file, Labels, removing Media files, and deleting Hidden entries.

use anyhow::Result;
use diesel::prelude::*;

use super::Catalogue;
use super::media_files::{self, READY};
use super::schema::media_file;

/// The outcome of an admin change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Done,
    /// No such Media file or entry.
    NotFound,
    /// The change breaks a rule of the catalogue; the message says which.
    Refused(String),
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

impl Catalogue {
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
