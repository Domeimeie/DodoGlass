//! An in-memory TMDB: the test adds the Movies, Shows, Seasons, Episodes
//! (and images) it knows, and can make it unreachable. It starts empty, so
//! every search finds nothing.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use asset_server::ports::tmdb::{
    EpisodeDetails, MovieDetails, MovieSummary, SeasonDetails, ShowDetails, ShowSummary, Tmdb,
    TmdbError,
};
use async_trait::async_trait;

#[derive(Default)]
pub struct FakeTmdb {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    movies: Vec<MovieDetails>,
    shows: Vec<ShowDetails>,
    seasons: HashMap<(i64, i32), SeasonDetails>,
    episodes: HashMap<(i64, i32, i32), EpisodeDetails>,
    images: HashMap<String, Vec<u8>>,
    unreachable: bool,
    calls: usize,
    searches: usize,
}

/// A Movie TMDB knows; `poster` is its image path (`/abc.jpg`).
pub fn movie(tmdb_id: i64, title: &str, year: i32) -> MovieDetails {
    MovieDetails {
        tmdb_id,
        title: title.into(),
        year: Some(year),
        imdb_id: Some(format!("tt{tmdb_id:07}")),
        overview: Some(format!("About {title}.")),
        poster_path: None,
    }
}

/// A Show TMDB knows, first aired in `year`, without a poster.
pub fn show(tmdb_id: i64, name: &str, year: i32) -> ShowDetails {
    ShowDetails {
        tmdb_id,
        name: name.into(),
        year: Some(year),
        imdb_id: Some(format!("tt{tmdb_id:07}")),
        overview: Some(format!("About {name}.")),
        poster_path: None,
    }
}

/// A Season TMDB knows, without a poster.
pub fn season(name: &str) -> SeasonDetails {
    SeasonDetails {
        name: Some(name.into()),
        overview: Some(format!("About {name}.")),
        poster_path: None,
    }
}

/// An Episode TMDB knows, without a still.
pub fn episode(tmdb_id: i64, name: &str, air_date: &str) -> EpisodeDetails {
    EpisodeDetails {
        tmdb_id,
        name: Some(name.into()),
        overview: Some(format!("About {name}.")),
        still_path: None,
        air_date: Some(air_date.into()),
        imdb_id: Some(format!("tt{tmdb_id:07}")),
    }
}

impl FakeTmdb {
    pub fn add_movie(&self, movie: MovieDetails) {
        self.state.lock().unwrap().movies.push(movie);
    }

    pub fn add_show(&self, show: ShowDetails) {
        self.state.lock().unwrap().shows.push(show);
    }

    /// Season `number` of the Show with TMDB id `show`.
    pub fn add_season(&self, show: i64, number: i32, season: SeasonDetails) {
        let mut state = self.state.lock().unwrap();
        state.seasons.insert((show, number), season);
    }

    /// Episode `number` of Season `season` of the Show with TMDB id `show`.
    pub fn add_episode(&self, show: i64, season: i32, number: i32, episode: EpisodeDetails) {
        let mut state = self.state.lock().unwrap();
        state.episodes.insert((show, season, number), episode);
    }

    pub fn add_image(&self, path: &str, bytes: &[u8]) {
        let mut state = self.state.lock().unwrap();
        state.images.insert(path.into(), bytes.to_vec());
    }

    /// While unreachable, every call fails as if TMDB were down.
    pub fn set_unreachable(&self, unreachable: bool) {
        self.state.lock().unwrap().unreachable = unreachable;
    }

    /// How many calls reached the fake so far (failed ones included).
    pub fn calls(&self) -> usize {
        self.state.lock().unwrap().calls
    }

    /// How many of them were searches (Movie or TV).
    pub fn searches(&self) -> usize {
        self.state.lock().unwrap().searches
    }

    fn call(&self) -> Result<std::sync::MutexGuard<'_, State>, TmdbError> {
        let mut state = self.state.lock().unwrap();
        state.calls += 1;
        if state.unreachable {
            return Err(TmdbError::Unavailable("connection refused".into()));
        }
        Ok(state)
    }

    fn search(&self) -> Result<std::sync::MutexGuard<'_, State>, TmdbError> {
        self.state.lock().unwrap().searches += 1;
        self.call()
    }
}

/// Lower case, letters and digits only: TMDB's search is just as loose.
fn words(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait]
impl Tmdb for FakeTmdb {
    /// Every Movie whose title contains the query's words, whatever its year:
    /// TMDB's `year` filter is lenient too (any release in that year).
    async fn search_movie(&self, name: &str, _year: i32) -> Result<Vec<MovieSummary>, TmdbError> {
        let state = self.search()?;
        let query = words(name);
        Ok(state
            .movies
            .iter()
            .filter(|m| words(&m.title).contains(&query))
            .map(|m| MovieSummary {
                tmdb_id: m.tmdb_id,
                title: m.title.clone(),
                year: m.year,
                overview: m.overview.clone(),
                poster_path: m.poster_path.clone(),
            })
            .collect())
    }

    async fn movie(&self, tmdb_id: i64) -> Result<MovieDetails, TmdbError> {
        let state = self.call()?;
        state
            .movies
            .iter()
            .find(|m| m.tmdb_id == tmdb_id)
            .cloned()
            .ok_or_else(|| TmdbError::Rejected(format!("no movie {tmdb_id}")))
    }

    /// Every Show whose name contains the query's words, whatever its year.
    async fn search_tv(&self, name: &str, _year: i32) -> Result<Vec<ShowSummary>, TmdbError> {
        let state = self.search()?;
        let query = words(name);
        Ok(state
            .shows
            .iter()
            .filter(|s| words(&s.name).contains(&query))
            .map(|s| ShowSummary {
                tmdb_id: s.tmdb_id,
                name: s.name.clone(),
                year: s.year,
                overview: s.overview.clone(),
                poster_path: s.poster_path.clone(),
            })
            .collect())
    }

    async fn tv(&self, tmdb_id: i64) -> Result<ShowDetails, TmdbError> {
        let state = self.call()?;
        state
            .shows
            .iter()
            .find(|s| s.tmdb_id == tmdb_id)
            .cloned()
            .ok_or_else(|| TmdbError::Rejected(format!("no show {tmdb_id}")))
    }

    async fn season(&self, tv_id: i64, number: i32) -> Result<Option<SeasonDetails>, TmdbError> {
        let state = self.call()?;
        Ok(state.seasons.get(&(tv_id, number)).cloned())
    }

    async fn episode(
        &self,
        tv_id: i64,
        season: i32,
        episode: i32,
    ) -> Result<Option<EpisodeDetails>, TmdbError> {
        let state = self.call()?;
        Ok(state.episodes.get(&(tv_id, season, episode)).cloned())
    }

    async fn image(&self, path: &str) -> Result<Vec<u8>, TmdbError> {
        let state = self.call()?;
        state
            .images
            .get(path)
            .cloned()
            .ok_or_else(|| TmdbError::Rejected(format!("no image {path}")))
    }
}
