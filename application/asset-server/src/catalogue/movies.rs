//! Movies, and what viewers see of them: only Movies with a Ready Media file
//! are listed; the others are Hidden.

use anyhow::Result;
use diesel::prelude::*;
use serde::Serialize;

use super::Catalogue;
use super::media_files::{Manifests, READY};
use super::schema::{media_file, movie};

#[derive(Debug, Clone, Serialize)]
pub struct MovieView {
    pub name: String,
    pub year: i32,
    pub media_files: Vec<MediaFileView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaFileView {
    pub id: String,
    pub label: String,
    pub manifests: Manifests,
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
struct ReadyMediaFileRow {
    id: String,
    label: String,
    hls_manifest: Option<String>,
    dash_manifest: Option<String>,
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
                            name: m.name,
                            year: m.year,
                            media_files: Vec::new(),
                        };
                        movies.push((m.id, view));
                    }
                    let (_, movie) = movies.last_mut().expect("just pushed");
                    movie.media_files.push(MediaFileView {
                        id: file.id,
                        label: file.label,
                        manifests: Manifests {
                            hls: file.hls_manifest,
                            dash: file.dash_manifest,
                        },
                    });
                }
                Ok(movies.into_iter().map(|(_, m)| m).collect())
            })
            .await
    }
}
