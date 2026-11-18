//! TMDB (The Movie Database), where descriptive information comes from.
//!
//! [`Tmdb`] is the port: one method per TMDB call the asset server makes.
//! [`TmdbClient`] is the real one (TMDB's v3 API over HTTPS); the tests use
//! an in-memory fake. Shows, Seasons and Episodes add their own calls here
//! (`search_tv`, `tv`, `season`, `episode`) next to the Movie ones.
//!
//! Errors say whether trying again later can help ([`TmdbError::Unavailable`]:
//! TMDB unreachable, rate limiting or failing) or not ([`TmdbError::Rejected`]).

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// A Movie as TMDB's search lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct MovieSummary {
    pub tmdb_id: i64,
    pub title: String,
    /// The year of the primary release date; `None` when TMDB has no date.
    pub year: Option<i32>,
    pub overview: Option<String>,
    /// TMDB's image path (`/abc.jpg`), for [`Tmdb::image`].
    pub poster_path: Option<String>,
}

/// A Movie with everything the catalogue takes from TMDB.
#[derive(Debug, Clone, PartialEq)]
pub struct MovieDetails {
    pub tmdb_id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub imdb_id: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TmdbError {
    /// Unreachable, rate limiting (429) or failing (5xx): worth retrying later.
    Unavailable(String),
    /// Anything else (unknown id, bad API key, unexpected answer).
    Rejected(String),
}

impl std::fmt::Display for TmdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TmdbError::Unavailable(e) => write!(f, "TMDB unavailable: {e}"),
            TmdbError::Rejected(e) => write!(f, "TMDB rejected the request: {e}"),
        }
    }
}

impl std::error::Error for TmdbError {}

#[async_trait]
pub trait Tmdb: Send + Sync + 'static {
    /// Movies whose title matches `name`, released in `year`, in TMDB's order.
    async fn search_movie(&self, name: &str, year: i32) -> Result<Vec<MovieSummary>, TmdbError>;

    /// The Movie with TMDB id `tmdb_id`.
    async fn movie(&self, tmdb_id: i64) -> Result<MovieDetails, TmdbError>;

    /// The image at `path` (`/abc.jpg`) in its original size.
    async fn image(&self, path: &str) -> Result<Vec<u8>, TmdbError>;
}

/// No TMDB (no API key configured): every call is unavailable. `main` does not
/// run the matcher then, so nothing calls it.
pub struct NoTmdb;

#[async_trait]
impl Tmdb for NoTmdb {
    async fn search_movie(&self, _: &str, _: i32) -> Result<Vec<MovieSummary>, TmdbError> {
        Err(TmdbError::Unavailable("no TMDB API key".into()))
    }

    async fn movie(&self, _: i64) -> Result<MovieDetails, TmdbError> {
        Err(TmdbError::Unavailable("no TMDB API key".into()))
    }

    async fn image(&self, _: &str) -> Result<Vec<u8>, TmdbError> {
        Err(TmdbError::Unavailable("no TMDB API key".into()))
    }
}

/// Settings of the real client.
#[derive(Debug, Clone)]
pub struct TmdbSettings {
    /// A v3 API key, or a v4 read access token (sent as a bearer token).
    pub api_key: String,
    /// The language names and descriptions are asked in, e.g. `en-US`.
    pub language: String,
    /// `https://api.themoviedb.org/3`
    pub api_url: String,
    /// `https://image.tmdb.org/t/p/original`
    pub image_url: String,
}

impl TmdbSettings {
    pub fn new(api_key: impl Into<String>, language: impl Into<String>) -> Self {
        TmdbSettings {
            api_key: api_key.into(),
            language: language.into(),
            api_url: "https://api.themoviedb.org/3".into(),
            image_url: "https://image.tmdb.org/t/p/original".into(),
        }
    }
}

/// TMDB's v3 API over HTTPS.
pub struct TmdbClient {
    http: reqwest::Client,
    settings: TmdbSettings,
}

impl TmdbClient {
    pub fn new(settings: TmdbSettings) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .user_agent(concat!(
                "dodoglass-asset-server/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()?;
        Ok(TmdbClient { http, settings })
    }

    /// v4 read access tokens are JWTs; v3 keys are 32 hex characters.
    fn is_bearer_token(&self) -> bool {
        self.settings.api_key.starts_with("eyJ")
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, TmdbError> {
        let mut url = Url::parse(&format!("{}{path}", self.settings.api_url))
            .map_err(|e| TmdbError::Rejected(format!("bad URL: {e}")))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("language", &self.settings.language);
            for (k, v) in params {
                query.append_pair(k, v);
            }
            if !self.is_bearer_token() {
                query.append_pair("api_key", &self.settings.api_key);
            }
        }
        let mut request = self.http.get(url);
        if self.is_bearer_token() {
            request = request.bearer_auth(&self.settings.api_key);
        }
        let bytes = send(request, path).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| TmdbError::Rejected(format!("{path}: unexpected answer: {e}")))
    }
}

/// Sends `request`; the body of a 2xx answer, or the error it means.
async fn send(request: reqwest::RequestBuilder, what: &str) -> Result<Vec<u8>, TmdbError> {
    let response = request
        .send()
        .await
        .map_err(|e| TmdbError::Unavailable(format!("{what}: {e}")))?;
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(TmdbError::Unavailable(format!("{what}: {status}")));
    }
    if !status.is_success() {
        return Err(TmdbError::Rejected(format!("{what}: {status}")));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| TmdbError::Unavailable(format!("{what}: {e}")))?;
    Ok(bytes.to_vec())
}

#[derive(Deserialize)]
struct SearchAnswer<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct MovieJson {
    id: i64,
    title: String,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    imdb_id: Option<String>,
}

/// `"1999-03-30"` → 1999; TMDB sends `""` when there is no date.
fn year_of(date: Option<&str>) -> Option<i32> {
    date?.get(..4)?.parse().ok()
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

impl From<MovieJson> for MovieDetails {
    fn from(m: MovieJson) -> Self {
        MovieDetails {
            tmdb_id: m.id,
            title: m.title,
            year: year_of(m.release_date.as_deref()),
            imdb_id: non_empty(m.imdb_id),
            overview: non_empty(m.overview),
            poster_path: non_empty(m.poster_path),
        }
    }
}

#[async_trait]
impl Tmdb for TmdbClient {
    async fn search_movie(&self, name: &str, year: i32) -> Result<Vec<MovieSummary>, TmdbError> {
        let year = year.to_string();
        let answer: SearchAnswer<MovieJson> = self
            .get_json(
                "/search/movie",
                &[("query", name), ("year", &year), ("include_adult", "false")],
            )
            .await?;
        Ok(answer
            .results
            .into_iter()
            .map(|m| {
                let m = MovieDetails::from(m);
                MovieSummary {
                    tmdb_id: m.tmdb_id,
                    title: m.title,
                    year: m.year,
                    overview: m.overview,
                    poster_path: m.poster_path,
                }
            })
            .collect())
    }

    async fn movie(&self, tmdb_id: i64) -> Result<MovieDetails, TmdbError> {
        let movie: MovieJson = self.get_json(&format!("/movie/{tmdb_id}"), &[]).await?;
        Ok(movie.into())
    }

    async fn image(&self, path: &str) -> Result<Vec<u8>, TmdbError> {
        if !path.starts_with('/') || path.contains("..") {
            return Err(TmdbError::Rejected(format!("bad image path {path:?}")));
        }
        let url = format!("{}{path}", self.settings.image_url);
        send(self.http.get(url), "image").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_come_from_release_dates() {
        assert_eq!(year_of(Some("1999-03-30")), Some(1999));
        assert_eq!(year_of(Some("")), None);
        assert_eq!(year_of(None), None);
    }

    #[test]
    fn a_v4_token_is_sent_as_bearer() {
        let client = |key: &str| TmdbClient::new(TmdbSettings::new(key, "en-US")).unwrap();
        assert!(client("eyJhbGciOiJIUzI1NiJ9.x.y").is_bearer_token());
        assert!(!client("0123456789abcdef0123456789abcdef").is_bearer_token());
    }
}
