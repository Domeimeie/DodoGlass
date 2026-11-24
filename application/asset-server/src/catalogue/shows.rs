//! Shows, Seasons and Episodes, and what viewers see of them: an Episode is
//! listed when it has a Ready Media file, a Season when it has a listed
//! Episode, a Show when it has a listed Season. The others are Hidden.
//!
//! Images fall back upwards: a Season without a poster shows its Show's, an
//! Episode without a still its Season's (so, failing that, its Show's).

use anyhow::Result;
use diesel::prelude::*;
use serde::Serialize;

use super::Catalogue;
use super::media_files::READY;
use super::movies::{MediaFileView, ReadyMediaFileRow, show_default};
use super::ratings::RatingView;
use super::schema::{episode, media_file, season, show};
use super::show_metadata::{self, ShowMetadataView};

#[derive(Debug, Clone, Serialize)]
pub struct ShowView {
    pub id: i32,
    pub name: String,
    pub year: i32,
    /// What a Match filled in (TMDB id, imdbID, description, poster).
    #[serde(flatten)]
    pub metadata: ShowMetadataView,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeasonView {
    pub number: i32,
    /// From TMDB, or [`season_name`] of the number.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its poster, or its Show's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpisodeView {
    pub number: i32,
    /// From TMDB, or [`episode_name`] of the number.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its still, or its Season's image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// ISO date, `2008-01-27`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(flatten)]
    pub rating: RatingView,
    pub media_files: Vec<MediaFileView>,
}

/// The name of a Season TMDB has no name for: "Season 2", or "Specials".
pub fn season_name(number: i32) -> String {
    if number == 0 {
        "Specials".into()
    } else {
        format!("Season {number}")
    }
}

/// The name of an Episode TMDB has no name for: "Episode 3".
pub fn episode_name(number: i32) -> String {
    format!("Episode {number}")
}

/// A Season as viewers see it, with its image falling back to `show_image`.
fn season_view(
    conn: &mut SqliteConnection,
    season_id: i32,
    number: i32,
    show_image: Option<String>,
) -> Result<SeasonView> {
    let row = show_metadata::season_rows(conn, &[season_id])?.pop();
    let (name, description, image) = match row {
        Some(r) => (r.name, r.description, r.image_key),
        None => (None, None, None),
    };
    Ok(SeasonView {
        number,
        name: name.unwrap_or_else(|| season_name(number)),
        description,
        image: image.or(show_image),
    })
}

/// The image of the Show `show` (when matched with a poster).
fn show_image(conn: &mut SqliteConnection, show: i32) -> Result<Option<String>> {
    Ok(show_metadata::show_rows(conn, &[show])?
        .pop()
        .and_then(|r| r.image_key))
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

/// The id of the Episode at `place`, if it exists (the Show by its folder
/// name and year).
pub(super) fn find(conn: &mut SqliteConnection, place: EpisodePlace) -> Result<Option<i32>> {
    let (Ok(season), Ok(number)) = (i32::try_from(place.season), i32::try_from(place.episode))
    else {
        return Ok(None);
    };
    Ok(episode::table
        .inner_join(season::table.inner_join(show::table))
        .filter(show::folder_name.eq(place.show))
        .filter(show::folder_year.eq(i32::from(place.year)))
        .filter(season::number.eq(season))
        .filter(episode::number.eq(number))
        .select(episode::id)
        .first(conn)
        .optional()?)
}

/// The id of Season `number` of Show `show`, if it exists.
pub(super) fn season_id(
    conn: &mut SqliteConnection,
    show: i32,
    number: i32,
) -> Result<Option<i32>> {
    Ok(season::table
        .filter(season::show_id.eq(show))
        .filter(season::number.eq(number))
        .select(season::id)
        .first(conn)
        .optional()?)
}

/// The id of Episode `number` of Season `season` of Show `show`, if it exists.
pub(super) fn episode_id(
    conn: &mut SqliteConnection,
    show: i32,
    season: i32,
    number: i32,
) -> Result<Option<i32>> {
    let Some(season) = season_id(conn, show, season)? else {
        return Ok(None);
    };
    Ok(episode::table
        .filter(episode::season_id.eq(season))
        .filter(episode::number.eq(number))
        .select(episode::id)
        .first(conn)
        .optional()?)
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
                let ids: Vec<i32> = rows.iter().map(|(id, _, _)| *id).collect();
                let mut metadata = show_metadata::show_rows(conn, &ids)?;
                Ok(rows
                    .into_iter()
                    .map(|(id, name, year)| {
                        let found = metadata.iter().position(|m| m.show_id == id);
                        ShowView {
                            id,
                            name,
                            year,
                            metadata: found
                                .map(|i| metadata.swap_remove(i).into())
                                .unwrap_or_default(),
                        }
                    })
                    .collect())
            })
            .await
    }

    /// The Seasons of Show `show` that are not Hidden, by number; `None` if
    /// the Show is unknown or Hidden.
    pub async fn visible_seasons(&self, show: i32) -> Result<Option<Vec<SeasonView>>> {
        self.db
            .run(move |conn| {
                let seasons: Vec<(i32, i32)> = season::table
                    .inner_join(episode::table.inner_join(media_file::table))
                    .filter(season::show_id.eq(show))
                    .filter(media_file::state.eq(READY))
                    .select((season::id, season::number))
                    .distinct()
                    .order(season::number.asc())
                    .load(conn)?;
                if seasons.is_empty() {
                    return Ok(None);
                }
                let show_image = show_image(conn, show)?;
                let views = seasons
                    .into_iter()
                    .map(|(id, number)| season_view(conn, id, number, show_image.clone()))
                    .collect::<Result<_>>()?;
                Ok(Some(views))
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
                let rows: Vec<(i32, i32, i32, ReadyMediaFileRow)> = episode::table
                    .inner_join(season::table)
                    .inner_join(media_file::table)
                    .filter(season::show_id.eq(show))
                    .filter(season::number.eq(season))
                    .filter(media_file::state.eq(READY))
                    .order((episode::number.asc(), media_file::label.asc()))
                    .select((
                        season::id,
                        episode::id,
                        episode::number,
                        ReadyMediaFileRow::as_select(),
                    ))
                    .load(conn)?;
                let Some(&(season_id, ..)) = rows.first() else {
                    return Ok(None);
                };
                let show_image = show_image(conn, show)?;
                let season_image = season_view(conn, season_id, season, show_image)?.image;
                let ids: Vec<i32> = rows.iter().map(|(_, id, _, _)| *id).collect();
                let metadata = show_metadata::episode_rows(conn, &ids)?;
                let mut episodes: Vec<(i32, EpisodeView)> = Vec::new();
                for (_, id, number, file) in rows {
                    if episodes.last().is_none_or(|(last, _)| *last != id) {
                        let m = metadata.iter().find(|m| m.episode_id == id);
                        let field =
                            |f: fn(&show_metadata::EpisodeMetadataRow) -> &Option<String>| {
                                m.and_then(|m| f(m).clone())
                            };
                        episodes.push((
                            id,
                            EpisodeView {
                                number,
                                name: field(|m| &m.name).unwrap_or_else(|| episode_name(number)),
                                tmdb_id: m.and_then(|m| m.tmdb_id),
                                imdb_id: field(|m| &m.imdb_id),
                                description: field(|m| &m.description),
                                image: field(|m| &m.image_key).or_else(|| season_image.clone()),
                                air_date: field(|m| &m.air_date),
                                rating: m
                                    .map(|m| {
                                        RatingView::from_columns(
                                            m.rating,
                                            m.votes,
                                            m.rating_updated_ms,
                                        )
                                    })
                                    .unwrap_or_default(),
                                media_files: Vec::new(),
                            },
                        ));
                    }
                    let (_, episode) = episodes.last_mut().expect("just pushed");
                    episode.media_files.push(file.into());
                }
                for (_, episode) in &mut episodes {
                    show_default(&mut episode.media_files);
                }
                Ok(Some(episodes.into_iter().map(|(_, e)| e).collect()))
            })
            .await
    }
}
