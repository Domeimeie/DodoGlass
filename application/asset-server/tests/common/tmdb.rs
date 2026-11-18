//! An in-memory TMDB: the test adds the Movies (and images) it knows, and can
//! make it unreachable. It starts empty, so every search finds nothing.

use std::collections::HashMap;
use std::sync::Mutex;

use asset_server::ports::tmdb::{MovieDetails, MovieSummary, Tmdb, TmdbError};
use async_trait::async_trait;

#[derive(Default)]
pub struct FakeTmdb {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    movies: Vec<MovieDetails>,
    images: HashMap<String, Vec<u8>>,
    unreachable: bool,
    calls: usize,
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

impl FakeTmdb {
    pub fn add_movie(&self, movie: MovieDetails) {
        self.state.lock().unwrap().movies.push(movie);
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

    fn call(&self) -> Result<std::sync::MutexGuard<'_, State>, TmdbError> {
        let mut state = self.state.lock().unwrap();
        state.calls += 1;
        if state.unreachable {
            return Err(TmdbError::Unavailable("connection refused".into()));
        }
        Ok(state)
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
        let state = self.call()?;
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

    async fn image(&self, path: &str) -> Result<Vec<u8>, TmdbError> {
        let state = self.call()?;
        state
            .images
            .get(path)
            .cloned()
            .ok_or_else(|| TmdbError::Rejected(format!("no image {path}")))
    }
}
