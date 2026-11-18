//! Shows, Seasons and Episodes, and what viewers see of them: an Episode is
//! listed when it has a Ready Media file, a Season when it has a listed
//! Episode, a Show when it has a listed Season. The others are Hidden.

use anyhow::Result;
use diesel::prelude::*;
use serde::Serialize;

use super::Catalogue;
use super::media_files::{Manifests, READY};
use super::movies::MediaFileView;
use super::schema::{episode, media_file, season, show};

#[derive(Debug, Clone, Serialize)]
pub struct ShowView {
    pub id: i32,
    pub name: String,
    pub year: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeasonView {
    pub number: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpisodeView {
    pub number: i32,
    pub media_files: Vec<MediaFileView>,
}

/// Where an Episode upload belongs: `shows/<show> (<year>)/Season <season>/S..E<episode>`.
pub(super) struct EpisodePlace<'a> {
    pub show: &'a str,
    pub year: u16,
    pub season: u32,
    pub episode: u32,
}

/// The id of the Episode at `place`, created with its Show and Season if new.
pub(super) fn find_or_create(conn: &mut SqliteConnection, place: EpisodePlace) -> Result<i32> {
    let year = i32::from(place.year);
    diesel::insert_into(show::table)
        .values((
            show::folder_name.eq(place.show),
            show::folder_year.eq(year),
            show::name.eq(place.show),
            show::year.eq(year),
        ))
        .on_conflict((show::folder_name, show::folder_year))
        .do_nothing()
        .execute(conn)?;
    let show_id: i32 = show::table
        .filter(show::folder_name.eq(place.show))
        .filter(show::folder_year.eq(year))
        .select(show::id)
        .first(conn)?;

    let number = i32::try_from(place.season)?;
    diesel::insert_into(season::table)
        .values((season::show_id.eq(show_id), season::number.eq(number)))
        .on_conflict((season::show_id, season::number))
        .do_nothing()
        .execute(conn)?;
    let season_id: i32 = season::table
        .filter(season::show_id.eq(show_id))
        .filter(season::number.eq(number))
        .select(season::id)
        .first(conn)?;

    let number = i32::try_from(place.episode)?;
    diesel::insert_into(episode::table)
        .values((episode::season_id.eq(season_id), episode::number.eq(number)))
        .on_conflict((episode::season_id, episode::number))
        .do_nothing()
        .execute(conn)?;
    Ok(episode::table
        .filter(episode::season_id.eq(season_id))
        .filter(episode::number.eq(number))
        .select(episode::id)
        .first(conn)?)
}

/// Media file `id` belongs to Episode `episode`.
pub(super) fn attach(conn: &mut SqliteConnection, id: &str, episode: i32) -> Result<()> {
    diesel::update(media_file::table.find(id))
        .set(media_file::episode_id.eq(episode))
        .execute(conn)?;
    Ok(())
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
    /// The Shows that are not Hidden, by name.
    pub async fn visible_shows(&self) -> Result<Vec<ShowView>> {
        self.db
            .run(|conn| {
                let rows: Vec<(i32, String, i32)> = show::table
                    .inner_join(
                        season::table.inner_join(episode::table.inner_join(media_file::table)),
                    )
                    .filter(media_file::state.eq(READY))
                    .select((show::id, show::name, show::year))
                    .distinct()
                    .order((show::name.asc(), show::year.asc(), show::id.asc()))
                    .load(conn)?;
                Ok(rows
                    .into_iter()
                    .map(|(id, name, year)| ShowView { id, name, year })
                    .collect())
            })
            .await
    }

    /// The Seasons of Show `show` that are not Hidden, by number; `None` if
    /// the Show is unknown or Hidden.
    pub async fn visible_seasons(&self, show: i32) -> Result<Option<Vec<SeasonView>>> {
        self.db
            .run(move |conn| {
                let numbers: Vec<i32> = season::table
                    .inner_join(episode::table.inner_join(media_file::table))
                    .filter(season::show_id.eq(show))
                    .filter(media_file::state.eq(READY))
                    .select(season::number)
                    .distinct()
                    .order(season::number.asc())
                    .load(conn)?;
                Ok((!numbers.is_empty()).then(|| {
                    numbers
                        .into_iter()
                        .map(|number| SeasonView { number })
                        .collect()
                }))
            })
            .await
    }

    /// The Episodes of Season `season` of Show `show` that are not Hidden, by
    /// number, each with its Ready Media files; `None` if the Season is
    /// unknown or Hidden.
    pub async fn visible_episodes(
        &self,
        show: i32,
        season: i32,
    ) -> Result<Option<Vec<EpisodeView>>> {
        self.db
            .run(move |conn| {
                let rows: Vec<(i32, ReadyMediaFileRow)> = episode::table
                    .inner_join(season::table)
                    .inner_join(media_file::table)
                    .filter(season::show_id.eq(show))
                    .filter(season::number.eq(season))
                    .filter(media_file::state.eq(READY))
                    .order((episode::number.asc(), media_file::label.asc()))
                    .select((episode::number, ReadyMediaFileRow::as_select()))
                    .load(conn)?;
                let mut episodes: Vec<EpisodeView> = Vec::new();
                for (number, file) in rows {
                    if episodes.last().is_none_or(|last| last.number != number) {
                        episodes.push(EpisodeView {
                            number,
                            media_files: Vec::new(),
                        });
                    }
                    let episode = episodes.last_mut().expect("just pushed");
                    episode.media_files.push(MediaFileView {
                        id: file.id,
                        label: file.label,
                        manifests: Manifests {
                            hls: file.hls_manifest,
                            dash: file.dash_manifest,
                        },
                    });
                }
                Ok((!episodes.is_empty()).then_some(episodes))
            })
            .await
    }
}
