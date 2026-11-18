//! TMDB (The Movie Database), where descriptive information comes from.
//!
//! [`Tmdb`] is the port: one method per TMDB call the asset server makes.
//! [`TmdbClient`] is the real one (TMDB's v3 API over HTTPS); the tests use
//! an in-memory fake. Movies have `search_movie` and `movie`; Shows,
//! Seasons and Episodes `search_tv`, `tv`, `season` and `episode`.
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

/// A Show as TMDB's search lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShowSummary {
    pub tmdb_id: i64,
    pub name: String,
    /// The year of the first air date; `None` when TMDB has no date.
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
}

/// A Show with everything the catalogue takes from TMDB.
#[derive(Debug, Clone, PartialEq)]
pub struct ShowDetails {
    pub tmdb_id: i64,
    pub name: String,
    pub year: Option<i32>,
    pub imdb_id: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
}

/// A Season with everything the catalogue takes from TMDB.
#[derive(Debug, Clone, PartialEq)]
pub struct SeasonDetails {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
}

/// An Episode with everything the catalogue takes from TMDB.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeDetails {
    pub tmdb_id: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    /// TMDB's image path of the still (`/abc.jpg`).
    pub still_path: Option<String>,
    /// ISO date, `2008-01-27`.
    pub air_date: Option<String>,
    pub imdb_id: Option<String>,
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

    /// Shows whose name matches `name`, first aired in `year`, in TMDB's order.
    async fn search_tv(&self, name: &str, year: i32) -> Result<Vec<ShowSummary>, TmdbError>;

    /// The Show with TMDB id `tmdb_id`.
    async fn tv(&self, tmdb_id: i64) -> Result<ShowDetails, TmdbError>;

    /// Season `number` of Show `tv_id`; `None` if TMDB does not know it.
    async fn season(&self, tv_id: i64, number: i32) -> Result<Option<SeasonDetails>, TmdbError>;

    /// Episode `episode` of Season `season` of Show `tv_id`; `None` if TMDB
    /// does not know it.
    async fn episode(
        &self,
        tv_id: i64,
        season: i32,
        episode: i32,
    ) -> Result<Option<EpisodeDetails>, TmdbError>;

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

    async fn search_tv(&self, _: &str, _: i32) -> Result<Vec<ShowSummary>, TmdbError> {
        Err(TmdbError::Unavailable("no TMDB API key".into()))
    }

    async fn tv(&self, _: i64) -> Result<ShowDetails, TmdbError> {
        Err(TmdbError::Unavailable("no TMDB API key".into()))
    }

    async fn season(&self, _: i64, _: i32) -> Result<Option<SeasonDetails>, TmdbError> {
        Err(TmdbError::Unavailable("no TMDB API key".into()))
    }

    async fn episode(&self, _: i64, _: i32, _: i32) -> Result<Option<EpisodeDetails>, TmdbError> {
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
        self.get_json_found(path, params)
            .await?
            .ok_or_else(|| TmdbError::Rejected(format!("{path}: {}", StatusCode::NOT_FOUND)))
    }

    /// As [`Self::get_json`], with TMDB's "not found" (404) as `None`.
    async fn get_json_found<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<Option<T>, TmdbError> {
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
        let Some(bytes) = send_found(request, path).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| TmdbError::Rejected(format!("{path}: unexpected answer: {e}")))
    }
}

/// Sends `request`; the body of a 2xx answer, or the error it means.
async fn send(request: reqwest::RequestBuilder, what: &str) -> Result<Vec<u8>, TmdbError> {
    send_found(request, what)
        .await?
        .ok_or_else(|| TmdbError::Rejected(format!("{what}: {}", StatusCode::NOT_FOUND)))
}

/// As [`send`], with a 404 as `None`.
async fn send_found(
    request: reqwest::RequestBuilder,
    what: &str,
) -> Result<Option<Vec<u8>>, TmdbError> {
    let response = request
        .send()
        .await
        .map_err(|e| TmdbError::Unavailable(format!("{what}: {e}")))?;
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(TmdbError::Unavailable(format!("{what}: {status}")));
    }
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(TmdbError::Rejected(format!("{what}: {status}")));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| TmdbError::Unavailable(format!("{what}: {e}")))?;
    Ok(Some(bytes.to_vec()))
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

#[derive(Deserialize, Default)]
struct ExternalIdsJson {
    #[serde(default)]
    imdb_id: Option<String>,
}

#[derive(Deserialize)]
struct ShowJson {
    id: i64,
    name: String,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    /// Only with `append_to_response=external_ids`.
    #[serde(default)]
    external_ids: Option<ExternalIdsJson>,
}

impl From<ShowJson> for ShowDetails {
    fn from(s: ShowJson) -> Self {
        ShowDetails {
            tmdb_id: s.id,
            name: s.name,
            year: year_of(s.first_air_date.as_deref()),
            imdb_id: non_empty(s.external_ids.unwrap_or_default().imdb_id),
            overview: non_empty(s.overview),
            poster_path: non_empty(s.poster_path),
        }
    }
}

#[derive(Deserialize)]
struct SeasonJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
}

#[derive(Deserialize)]
struct EpisodeJson {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    still_path: Option<String>,
    #[serde(default)]
    air_date: Option<String>,
    #[serde(default)]
    external_ids: Option<ExternalIdsJson>,
}

impl From<EpisodeJson> for EpisodeDetails {
    fn from(e: EpisodeJson) -> Self {
        EpisodeDetails {
            tmdb_id: e.id,
            name: non_empty(e.name),
            overview: non_empty(e.overview),
            still_path: non_empty(e.still_path),
            air_date: non_empty(e.air_date),
            imdb_id: non_empty(e.external_ids.unwrap_or_default().imdb_id),
        }
    }
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

    async fn search_tv(&self, name: &str, year: i32) -> Result<Vec<ShowSummary>, TmdbError> {
        let year = year.to_string();
        let answer: SearchAnswer<ShowJson> = self
            .get_json(
                "/search/tv",
                &[
                    ("query", name),
                    ("first_air_date_year", &year),
                    ("include_adult", "false"),
                ],
            )
            .await?;
        Ok(answer
            .results
            .into_iter()
            .map(|s| {
                let s = ShowDetails::from(s);
                ShowSummary {
                    tmdb_id: s.tmdb_id,
                    name: s.name,
                    year: s.year,
                    overview: s.overview,
                    poster_path: s.poster_path,
                }
            })
            .collect())
    }

    async fn tv(&self, tmdb_id: i64) -> Result<ShowDetails, TmdbError> {
        let show: ShowJson = self
            .get_json(
                &format!("/tv/{tmdb_id}"),
                &[("append_to_response", "external_ids")],
            )
            .await?;
        Ok(show.into())
    }

    async fn season(&self, tv_id: i64, number: i32) -> Result<Option<SeasonDetails>, TmdbError> {
        let season: Option<SeasonJson> = self
            .get_json_found(&format!("/tv/{tv_id}/season/{number}"), &[])
            .await?;
        Ok(season.map(|s| SeasonDetails {
            name: non_empty(s.name),
            overview: non_empty(s.overview),
            poster_path: non_empty(s.poster_path),
        }))
    }

    async fn episode(
        &self,
        tv_id: i64,
        season: i32,
        episode: i32,
    ) -> Result<Option<EpisodeDetails>, TmdbError> {
        let episode: Option<EpisodeJson> = self
            .get_json_found(
                &format!("/tv/{tv_id}/season/{season}/episode/{episode}"),
                &[("append_to_response", "external_ids")],
            )
            .await?;
        Ok(episode.map(EpisodeDetails::from))
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
    fn imdb_ids_of_shows_and_episodes_come_from_external_ids() {
        let show = r#"{"id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20",
            "overview": "", "poster_path": "/p.jpg", "external_ids": {"imdb_id": "tt0903747"}}"#;
        let show = ShowDetails::from(serde_json::from_str::<ShowJson>(show).unwrap());
        assert_eq!(show.year, Some(2008));
        assert_eq!(show.imdb_id.as_deref(), Some("tt0903747"));
        assert_eq!(show.overview, None);

        let episode = r#"{"id": 62086, "name": "Cat's in the Bag...", "air_date": "2008-01-27",
            "still_path": null, "external_ids": {"imdb_id": "tt1054724"}}"#;
        let episode = EpisodeDetails::from(serde_json::from_str::<EpisodeJson>(episode).unwrap());
        assert_eq!(episode.imdb_id.as_deref(), Some("tt1054724"));
        assert_eq!(episode.air_date.as_deref(), Some("2008-01-27"));
        assert_eq!(episode.still_path, None);
    }

    #[test]
    fn a_v4_token_is_sent_as_bearer() {
        let client = |key: &str| TmdbClient::new(TmdbSettings::new(key, "en-US")).unwrap();
        assert!(client("eyJhbGciOiJIUzI1NiJ9.x.y").is_bearer_token());
        assert!(!client("0123456789abcdef0123456789abcdef").is_bearer_token());
    }
}
