//! Movies, and what viewers see of them: only Movies with a Ready Media file
//! are listed; the others are Hidden.

use anyhow::Result;
use diesel::prelude::*;
use serde::Serialize;

use super::Catalogue;
use super::media_files::{Manifests, READY};
use super::metadata::{self, MovieMetadataView};
use super::schema::{media_file, movie};

#[derive(Debug, Clone, Serialize)]
pub struct MovieView {
    pub id: i32,
    pub name: String,
    pub year: i32,
    pub media_files: Vec<MediaFileView>,
    /// What a Match filled in (TMDB id, imdbID, description, poster).
    #[serde(flatten)]
    pub metadata: MovieMetadataView,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaFileView {
    pub id: String,
    pub label: String,
    /// Whether this is the Default media file of its Movie or Episode, as
    /// viewers see it (see [`show_default`]).
    pub default: bool,
    pub manifests: Manifests,
    /// When it was first placed: the oldest one stands in for the default.
    #[serde(skip)]
    created_ms: i64,
}

impl From<ReadyMediaFileRow> for MediaFileView {
    fn from(row: ReadyMediaFileRow) -> Self {
        MediaFileView {
            id: row.id,
            label: row.label,
            default: row.is_default,
            manifests: Manifests {
                hls: row.hls_manifest,
                dash: row.dash_manifest,
            },
            created_ms: row.created_ms,
        }
    }
}

/// The Ready Media files of one Movie or Episode, as viewers see them. While
/// the stored Default media file is not among them (Missing, for instance
/// while a re-upload of it is transcoded), the oldest Ready one is shown as
/// the default instead. That choice is not stored: once the stored default is
/// Ready again, it is the default again (ADR 0006).
pub(super) fn show_default(files: &mut [MediaFileView]) {
    if files.iter().any(|f| f.default) {
        return;
    }
    if let Some(oldest) = files
        .iter_mut()
        .min_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)))
    {
        oldest.default = true;
    }
}

/// The id of the Movie from the folder `<name> (<year>)`, if it exists.
pub(super) fn find(conn: &mut SqliteConnection, name: &str, year: i32) -> Result<Option<i32>> {
    Ok(movie::table
        .filter(movie::folder_name.eq(name))
        .filter(movie::folder_year.eq(year))
        .select(movie::id)
        .first(conn)
        .optional()?)
}

/// The id of the Movie from the folder `<name> (<year>)`, created if new.
pub(super) fn find_or_create(conn: &mut SqliteConnection, name: &str, year: u16) -> Result<i32> {
    let year = i32::from(year);
    diesel::insert_into(movie::table)
        .values((
            movie::folder_name.eq(name),
            movie::folder_year.eq(year),
            movie::name.eq(name),
            movie::year.eq(year),
        ))
        .on_conflict((movie::folder_name, movie::folder_year))
        .do_nothing()
        .execute(conn)?;
    Ok(movie::table
        .filter(movie::folder_name.eq(name))
        .filter(movie::folder_year.eq(year))
        .select(movie::id)
        .first(conn)?)
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = movie)]
struct MovieRow {
    id: i32,
    name: String,
    year: i32,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = media_file)]
pub(super) struct ReadyMediaFileRow {
    id: String,
    label: String,
    is_default: bool,
    hls_manifest: Option<String>,
    dash_manifest: Option<String>,
    created_ms: i64,
}

impl Catalogue {
    /// The Movies that are not Hidden, by name, each with its Ready Media files.
    pub async fn visible_movies(&self) -> Result<Vec<MovieView>> {
        self.db
            .run(|conn| {
                let rows: Vec<(MovieRow, ReadyMediaFileRow)> = movie::table
                    .inner_join(media_file::table)
                    .filter(media_file::state.eq(READY))
                    .order((
                        movie::name.asc(),
                        movie::year.asc(),
                        movie::id.asc(),
                        media_file::label.asc(),
                    ))
                    .select((MovieRow::as_select(), ReadyMediaFileRow::as_select()))
                    .load(conn)?;
                let mut movies: Vec<(i32, MovieView)> = Vec::new();
                for (m, file) in rows {
                    if movies.last().is_none_or(|(last, _)| *last != m.id) {
                        let view = MovieView {
                            id: m.id,
                            name: m.name,
                            year: m.year,
                            media_files: Vec::new(),
                            metadata: MovieMetadataView::default(),
                        };
                        movies.push((m.id, view));
                    }
                    let (_, movie) = movies.last_mut().expect("just pushed");
                    movie.media_files.push(file.into());
                }
                for (_, movie) in &mut movies {
                    show_default(&mut movie.media_files);
                }
                metadata::fill_views(conn, &mut movies)?;
                Ok(movies.into_iter().map(|(_, m)| m).collect())
            })
            .await
    }
}
