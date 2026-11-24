//! Media file states. The catalogue only learns of a Media file once its
//! transcode is complete, from its output marker in `final` (ADR 0001, ADR
//! 0006), so a Media file is Ready from the start. It is Missing while its
//! marker is gone from `final` (its output was deleted, or a re-upload is
//! being transcoded), and Ready again once the marker is back.

use anyhow::Result;
use diesel::prelude::*;
use diesel::sqlite::Sqlite;
use serde::Serialize;

use super::Catalogue;
use super::schema::media_file;

pub(super) const READY: &str = "ready";
pub(super) const MISSING: &str = "missing";

/// A Media file's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaFileState {
    Ready,
    Missing,
}

impl MediaFileState {
    /// `ready` → Ready, as the API spells states.
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            READY => MediaFileState::Ready,
            MISSING => MediaFileState::Missing,
            other => anyhow::bail!("unknown Media file state {other:?}"),
        })
    }
}

impl MediaFileState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            MediaFileState::Ready => READY,
            MediaFileState::Missing => MISSING,
        }
    }
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = media_file)]
struct StatusRow {
    id: String,
    source_key: String,
    label: String,
    state: String,
}

impl StatusRow {
    fn into_status(self) -> Result<MediaFileStatus> {
        Ok(MediaFileStatus {
            id: self.id,
            key: self.source_key,
            label: self.label,
            state: MediaFileState::parse(&self.state)?,
        })
    }
}

/// A Media file the reconciler checks against `final`.
#[derive(Debug, Clone)]
pub struct TrackedMediaFile {
    pub id: String,
    pub state: MediaFileState,
    /// The manifests stored for it, to compare with its marker's formats.
    pub manifests: Manifests,
}

/// The state of one Media file, for the admin API.
#[derive(Debug, Clone, Serialize)]
pub struct MediaFileStatus {
    pub id: String,
    /// The upload key, as the output marker records it.
    pub key: String,
    pub label: String,
    pub state: MediaFileState,
}

/// The manifest keys of a Ready Media file (`<final bucket>/<id>/...`), for
/// the formats it was transcoded to.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Manifests {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hls: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dash: Option<String>,
}

/// What a Media file belongs to. Labels are unique, and there is one Default
/// media file, per owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    Movie(i32),
    Episode(i32),
}

impl Owner {
    pub(super) fn of(movie: Option<i32>, episode: Option<i32>) -> Option<Owner> {
        match (movie, episode) {
            (Some(movie), _) => Some(Owner::Movie(movie)),
            (None, Some(episode)) => Some(Owner::Episode(episode)),
            (None, None) => None,
        }
    }

    /// The Media files of this owner.
    fn files(self) -> media_file::BoxedQuery<'static, Sqlite> {
        match self {
            Owner::Movie(movie) => media_file::table
                .filter(media_file::movie_id.eq(movie))
                .into_boxed(),
            Owner::Episode(episode) => media_file::table
                .filter(media_file::episode_id.eq(episode))
                .into_boxed(),
        }
    }
}

/// The owner of Media file `id` and whether it is the owner's default;
/// `None` if there is no such Media file.
pub(super) fn owner_of(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<(Option<Owner>, bool)>> {
    let row: Option<(Option<i32>, Option<i32>, bool)> = media_file::table
        .find(id)
        .select((
            media_file::movie_id,
            media_file::episode_id,
            media_file::is_default,
        ))
        .first(conn)
        .optional()?;
    Ok(row.map(|(movie, episode, is_default)| (Owner::of(movie, episode), is_default)))
}

/// Makes Media file `id` the Default media file of `owner`, in place of the
/// current one.
pub(super) fn make_default(conn: &mut SqliteConnection, owner: Owner, id: &str) -> Result<()> {
    // The old default first: the unique index allows one default per owner.
    match owner {
        Owner::Movie(movie) => diesel::update(media_file::table)
            .filter(media_file::movie_id.eq(movie))
            .set(media_file::is_default.eq(false))
            .execute(conn)?,
        Owner::Episode(episode) => diesel::update(media_file::table)
            .filter(media_file::episode_id.eq(episode))
            .set(media_file::is_default.eq(false))
            .execute(conn)?,
    };
    diesel::update(media_file::table.find(id))
        .set(media_file::is_default.eq(true))
        .execute(conn)?;
    Ok(())
}

/// The oldest Media file of `owner` other than `except`, Ready ones only if
/// `ready_only`.
fn oldest(
    conn: &mut SqliteConnection,
    owner: Owner,
    except: &str,
    ready_only: bool,
) -> Result<Option<String>> {
    let mut query = owner.files().filter(media_file::id.ne(except.to_string()));
    if ready_only {
        query = query.filter(media_file::state.eq(READY));
    }
    Ok(query
        .order((media_file::created_ms.asc(), media_file::id.asc()))
        .select(media_file::id)
        .first(conn)
        .optional()?)
}

/// Media file `id` is about to be removed: if it is its owner's default, the
/// oldest other Ready Media file takes over, or else the oldest other one.
pub(super) fn hand_over_default_before_removal(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<()> {
    let Some((Some(owner), true)) = owner_of(conn, id)? else {
        return Ok(());
    };
    let next = match oldest(conn, owner, id, true)? {
        Some(next) => Some(next),
        None => oldest(conn, owner, id, false)?,
    };
    if let Some(next) = next {
        make_default(conn, owner, &next)?;
    }
    Ok(())
}

/// The Labels of `owner`'s Media files other than `except`.
pub(super) fn other_labels(
    conn: &mut SqliteConnection,
    owner: Owner,
    except: &str,
) -> Result<Vec<String>> {
    Ok(owner
        .files()
        .filter(media_file::id.ne(except.to_string()))
        .select(media_file::label)
        .load(conn)?)
}

/// Creates the Media file `id` as Ready with `manifests`. If it exists
/// already (a re-upload to the same path, or the same marker seen twice), it
/// is made Ready with them instead: its owner, Label and default flag stay.
///
/// A new Media file of `owner` is labelled `stem`, made unique within the
/// owner (see [`unique_label`]); the first one of an owner is its Default
/// media file. Run it inside the transaction that found the owner.
pub(super) fn place_ready(
    conn: &mut SqliteConnection,
    placed: Placed<'_>,
    manifests: Manifests,
    now: i64,
) -> Result<()> {
    let Placed {
        id,
        key,
        owner,
        stem,
    } = placed;
    if set_ready(conn, id, Some(key), &manifests, now)? {
        return Ok(());
    }
    let (label, is_default) = match owner {
        Some(owner) => {
            let taken = labels_of(conn, owner)?;
            (unique_label(stem, extension(key), &taken), taken.is_empty())
        }
        None => (stem.to_string(), false),
    };
    let (movie, episode) = match owner {
        Some(Owner::Movie(movie)) => (Some(movie), None),
        Some(Owner::Episode(episode)) => (None, Some(episode)),
        None => (None, None),
    };
    diesel::insert_into(media_file::table)
        .values((
            media_file::id.eq(id),
            media_file::source_key.eq(key),
            media_file::movie_id.eq(movie),
            media_file::episode_id.eq(episode),
            media_file::label.eq(label),
            media_file::is_default.eq(is_default),
            media_file::state.eq(READY),
            media_file::state_since_ms.eq(now),
            media_file::created_ms.eq(now),
            media_file::hls_manifest.eq(manifests.hls),
            media_file::dash_manifest.eq(manifests.dash),
        ))
        .execute(conn)?;
    Ok(())
}

/// Where a marker's input key places its Media file.
pub(super) struct Placed<'a> {
    pub id: &'a str,
    /// The input key, byte for byte as uploaded.
    pub key: &'a str,
    pub owner: Option<Owner>,
    /// The file name's stem, the Label it starts from.
    pub stem: &'a str,
}

/// Makes Media file `id` Ready with `manifests` (and, given one, the input
/// key's latest spelling). Its state's time only changes if it was not Ready.
/// `false` if there is no such Media file.
fn set_ready(
    conn: &mut SqliteConnection,
    id: &str,
    key: Option<&str>,
    manifests: &Manifests,
    now: i64,
) -> Result<bool> {
    let found = diesel::update(media_file::table.find(id))
        .set((
            media_file::hls_manifest.eq(&manifests.hls),
            media_file::dash_manifest.eq(&manifests.dash),
        ))
        .execute(conn)?;
    if found == 0 {
        return Ok(false);
    }
    if let Some(key) = key {
        // NFC and NFD spellings of a key have one id: the latest one counts.
        diesel::update(media_file::table.find(id))
            .set(media_file::source_key.eq(key))
            .execute(conn)?;
    }
    diesel::update(media_file::table.find(id))
        .filter(media_file::state.ne(READY))
        .set((
            media_file::state.eq(READY),
            media_file::state_since_ms.eq(now),
        ))
        .execute(conn)?;
    Ok(true)
}

/// The Unassigned media file `id` now belongs to `owner`. Its Label (the file
/// name's stem) is made unique within the owner as for an upload, and it is
/// the owner's Default media file only if the owner had none. Returns its new
/// Label, or `None` if `id` is not an Unassigned media file. Run it inside a
/// transaction.
pub(super) fn attach(
    conn: &mut SqliteConnection,
    id: &str,
    owner: Owner,
) -> Result<Option<String>> {
    let found: Option<(String, String)> = media_file::table
        .find(id)
        .filter(media_file::movie_id.is_null())
        .filter(media_file::episode_id.is_null())
        .select((media_file::source_key, media_file::label))
        .first(conn)
        .optional()?;
    let Some((key, stem)) = found else {
        return Ok(None);
    };
    let taken = labels_of(conn, owner)?;
    let label = unique_label(&stem, extension(&key), &taken);
    let (movie, episode) = match owner {
        Owner::Movie(movie) => (Some(movie), None),
        Owner::Episode(episode) => (None, Some(episode)),
    };
    diesel::update(media_file::table.find(id))
        .set((
            media_file::movie_id.eq(movie),
            media_file::episode_id.eq(episode),
            media_file::label.eq(&label),
            media_file::is_default.eq(taken.is_empty()),
        ))
        .execute(conn)?;
    Ok(Some(label))
}

/// The Labels of every Media file of `owner`, whatever its state.
fn labels_of(conn: &mut SqliteConnection, owner: Owner) -> Result<Vec<String>> {
    Ok(owner.files().select(media_file::label).load(conn)?)
}

/// `stem` if no other Media file of the owner has it, else `stem (ext)`
/// (`matrix.mkv` + `matrix.mp4` → "matrix", "matrix (mp4)"), with a number
/// added if that is taken too.
fn unique_label(stem: &str, ext: &str, taken: &[String]) -> String {
    let free = |label: &String| !taken.contains(label);
    let mut candidates = std::iter::once(stem.to_string())
        .chain((!ext.is_empty()).then(|| format!("{stem} ({ext})")))
        .chain((2..).map(|n| match ext {
            "" => format!("{stem} ({n})"),
            ext => format!("{stem} ({ext} {n})"),
        }));
    candidates
        .find(free)
        .expect("the numbered candidates never end")
}

/// `movies/The Matrix (1999)/matrix.mkv` → `mkv`; empty without one.
fn extension(key: &str) -> &str {
    let name = key.rsplit('/').next().unwrap_or(key);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext,
        _ => "",
    }
}

impl Catalogue {
    /// Every Media file, with what the reconciler needs to check it.
    pub async fn tracked_media_files(&self) -> Result<Vec<TrackedMediaFile>> {
        self.tracked(None).await
    }

    /// Media file `id`, with what the reconciler needs to check it.
    pub async fn tracked_media_file(&self, id: &str) -> Result<Option<TrackedMediaFile>> {
        Ok(self.tracked(Some(id.to_string())).await?.pop())
    }

    async fn tracked(&self, only: Option<String>) -> Result<Vec<TrackedMediaFile>> {
        self.db
            .run(move |conn| {
                let mut query = media_file::table.into_boxed();
                if let Some(id) = only {
                    query = query.filter(media_file::id.eq(id));
                }
                let rows: Vec<(String, String, Option<String>, Option<String>)> = query
                    .select((
                        media_file::id,
                        media_file::state,
                        media_file::hls_manifest,
                        media_file::dash_manifest,
                    ))
                    .load(conn)?;
                rows.into_iter()
                    .map(|(id, state, hls, dash)| {
                        Ok(TrackedMediaFile {
                            id,
                            state: MediaFileState::parse(&state)?,
                            manifests: Manifests { hls, dash },
                        })
                    })
                    .collect()
            })
            .await
    }

    /// The state of Media file `id`, if it exists.
    pub async fn media_file_status(&self, id: &str) -> Result<Option<MediaFileStatus>> {
        let id = id.to_string();
        self.db
            .run(move |conn| {
                let row: Option<StatusRow> = media_file::table
                    .find(&id)
                    .select(StatusRow::as_select())
                    .first(conn)
                    .optional()?;
                row.map(StatusRow::into_status).transpose()
            })
            .await
    }

    /// The Media files in any of `states`, by upload key.
    pub async fn media_files_in(
        &self,
        states: Vec<MediaFileState>,
    ) -> Result<Vec<MediaFileStatus>> {
        let states: Vec<&'static str> = states.into_iter().map(MediaFileState::as_str).collect();
        self.db
            .run(move |conn| {
                let rows: Vec<StatusRow> = media_file::table
                    .filter(media_file::state.eq_any(states))
                    .order(media_file::source_key.asc())
                    .select(StatusRow::as_select())
                    .load(conn)?;
                rows.into_iter().map(StatusRow::into_status).collect()
            })
            .await
    }

    /// The output of Ready Media file `id` is gone from `final`: it is
    /// Missing until it comes back. It is never deleted automatically.
    /// Its Label stays, and so does the Default media file: while it is
    /// Missing the read API shows another Ready one as default (ADR 0006).
    pub async fn output_lost(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                diesel::update(media_file::table.find(&id))
                    .filter(media_file::state.eq(READY))
                    .set((
                        media_file::state.eq(MISSING),
                        media_file::state_since_ms.eq(now),
                    ))
                    .execute(conn)?;
                Ok(())
            })
            .await
    }

    /// The output marker of known Media file `id` was found: it is Ready
    /// (again), with these manifests. The marker is the source of truth
    /// (ADR 0001).
    pub async fn output_found(&self, id: &str, manifests: Manifests) -> Result<()> {
        let id = id.to_string();
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                set_ready(conn, &id, None, &manifests, now)?;
                Ok(())
            })
            .await
    }
}
