//! What an admin changes in the catalogue: which Media file is the Default
//! media file, Labels, removing Media files, and deleting Hidden entries.

use anyhow::Result;
use diesel::prelude::*;

use super::Catalogue;
use super::media_files::{self, READY};
use super::schema::media_file;
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

impl Catalogue {
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
