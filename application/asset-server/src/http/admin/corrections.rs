//! Admin API: metadata corrections. Picking a Match for a Movie or Show (one
//! of its candidates, or any TMDB id), refreshing an entry from TMDB, editing
//! fields by hand (which locks them) and unlocking them. The paths follow the read API's.
//!
//! TMDB refusing (no such id) is a 400, TMDB being unavailable a 503.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::catalogue::{Catalogue, Edited, Entry};
use crate::http::{ApiError, ApiState};
use crate::matcher::{Corrected, Matcher};

const MOVIE: &str = "/movies/{movie}";
const SHOW: &str = "/shows/{show}";
const SEASON: &str = "/shows/{show}/seasons/{season}";
const EPISODE: &str = "/shows/{show}/seasons/{season}/episodes/{episode}";

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(&format!("{MOVIE}/match"), put(pick_movie_match))
        .route(&format!("{MOVIE}/refresh"), post(refresh_movie))
        .route(&format!("{SHOW}/match"), put(pick_show_match))
        .route(&format!("{SHOW}/refresh"), post(refresh_show))
        .route(&format!("{SEASON}/refresh"), post(refresh_season))
        .route(&format!("{EPISODE}/refresh"), post(refresh_episode))
        .route(MOVIE, patch(edit_movie))
        .route(SHOW, patch(edit_show))
        .route(SEASON, patch(edit_season))
        .route(EPISODE, patch(edit_episode))
        .route(&format!("{MOVIE}/locks/{{field}}"), delete(unlock_movie))
        .route(&format!("{SHOW}/locks/{{field}}"), delete(unlock_show))
        .route(&format!("{SEASON}/locks/{{field}}"), delete(unlock_season))
        .route(
            &format!("{EPISODE}/locks/{{field}}"),
            delete(unlock_episode),
        )
}

/// A [`Corrected`] as an answer: 204, or as [`failure`].
fn answer(corrected: Corrected) -> Result<StatusCode, ApiError> {
    match corrected {
        Corrected::Done => Ok(StatusCode::NO_CONTENT),
        other => Err(failure(other)),
    }
}

/// A [`Corrected`] that is not done as an error: 404, 409, 400 (TMDB
/// refused) or 503 (TMDB unavailable).
pub(super) fn failure(corrected: Corrected) -> ApiError {
    match corrected {
        Corrected::Done => anyhow::anyhow!("a correction that was done is no failure").into(),
        Corrected::NotFound(message) => ApiError::not_found(message),
        Corrected::Refused(message) => ApiError::conflict(message),
        Corrected::Rejected(message) => ApiError::bad_request(message),
        Corrected::Unavailable(message) => ApiError::unavailable(message),
    }
}

#[derive(Deserialize)]
struct Pick {
    tmdb_id: i64,
}

/// `PUT /admin/movies/<id>/match` with `{"tmdb_id"}`: makes that TMDB Movie
/// its Match; every field that is not locked is refilled.
async fn pick_movie_match(
    State(matcher): State<Matcher>,
    Path(movie): Path<i32>,
    Json(pick): Json<Pick>,
) -> Result<StatusCode, ApiError> {
    answer(matcher.pick_movie_match(movie, pick.tmdb_id).await?)
}

/// `POST /admin/movies/<id>/refresh`: fetches it from TMDB again (by its
/// Match, or by a new search if it has none); Locked fields are kept.
async fn refresh_movie(
    State(matcher): State<Matcher>,
    Path(movie): Path<i32>,
) -> Result<StatusCode, ApiError> {
    answer(matcher.refresh_movie(movie).await?)
}

/// `PUT /admin/shows/<id>/match` with `{"tmdb_id"}`: makes that TMDB Show
/// its Match; every field that is not locked is refilled, and its Seasons
/// and Episodes are looked up again by number (what they had from the old
/// Match is cleared, Locked fields kept).
async fn pick_show_match(
    State(matcher): State<Matcher>,
    Path(show): Path<i32>,
    Json(pick): Json<Pick>,
) -> Result<StatusCode, ApiError> {
    answer(matcher.pick_show_match(show, pick.tmdb_id).await?)
}

/// `POST /admin/shows/<id>/refresh`: as for a Movie, with its Seasons and
/// Episodes.
async fn refresh_show(
    State(matcher): State<Matcher>,
    Path(show): Path<i32>,
) -> Result<StatusCode, ApiError> {
    answer(matcher.refresh_show(show).await?)
}

/// `POST /admin/shows/<id>/seasons/<n>/refresh`: looks the Season up again
/// by number (not its Episodes); 409 if the Show has no Match.
async fn refresh_season(
    State(matcher): State<Matcher>,
    Path((show, number)): Path<(i32, i32)>,
) -> Result<StatusCode, ApiError> {
    answer(matcher.refresh_part(Entry::Season { show, number }).await?)
}

/// `POST /admin/shows/<id>/seasons/<n>/episodes/<n>/refresh`: as for a
/// Season.
async fn refresh_episode(
    State(matcher): State<Matcher>,
    Path((show, season, number)): Path<(i32, i32, i32)>,
) -> Result<StatusCode, ApiError> {
    let entry = Entry::Episode {
        show,
        season,
        number,
    };
    answer(matcher.refresh_part(entry).await?)
}

/// An [`Edited`] as an answer: `{"locked_fields": [...]}`, 404 or 400.
fn edited(edited: Edited) -> Result<Json<Value>, ApiError> {
    match edited {
        Edited::Locked(fields) => Ok(Json(json!({ "locked_fields": fields }))),
        Edited::NotFound => Err(ApiError::not_found("no such entry")),
        Edited::Invalid(message) => Err(ApiError::bad_request(message)),
    }
}

async fn edit(
    catalogue: Catalogue,
    entry: Entry,
    fields: Map<String, Value>,
) -> Result<Json<Value>, ApiError> {
    edited(catalogue.edit_fields(entry, fields).await?)
}

/// `PATCH /admin/movies/<id>` with fields to set, e.g. `{"description":
/// "..."}`: sets and locks them. Answers the Locked fields.
async fn edit_movie(
    State(catalogue): State<Catalogue>,
    Path(movie): Path<i32>,
    Json(fields): Json<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    edit(catalogue, Entry::Movie(movie), fields).await
}

/// `PATCH /admin/shows/<id>`: as for a Movie.
async fn edit_show(
    State(catalogue): State<Catalogue>,
    Path(show): Path<i32>,
    Json(fields): Json<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    edit(catalogue, Entry::Show(show), fields).await
}

/// `PATCH /admin/shows/<id>/seasons/<n>`: as for a Movie.
async fn edit_season(
    State(catalogue): State<Catalogue>,
    Path((show, number)): Path<(i32, i32)>,
    Json(fields): Json<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    edit(catalogue, Entry::Season { show, number }, fields).await
}

/// `PATCH /admin/shows/<id>/seasons/<n>/episodes/<n>`: as for a Movie.
async fn edit_episode(
    State(catalogue): State<Catalogue>,
    Path((show, season, number)): Path<(i32, i32, i32)>,
    Json(fields): Json<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    let entry = Entry::Episode {
        show,
        season,
        number,
    };
    edit(catalogue, entry, fields).await
}

/// `DELETE /admin/movies/<id>/locks/<field>`: unlocks the field; the next
/// Match or refresh fills it from TMDB. Answers the Locked fields left.
async fn unlock_movie(
    State(catalogue): State<Catalogue>,
    Path((movie, field)): Path<(i32, String)>,
) -> Result<Json<Value>, ApiError> {
    edited(catalogue.unlock_field(Entry::Movie(movie), field).await?)
}

/// `DELETE /admin/shows/<id>/locks/<field>`.
async fn unlock_show(
    State(catalogue): State<Catalogue>,
    Path((show, field)): Path<(i32, String)>,
) -> Result<Json<Value>, ApiError> {
    edited(catalogue.unlock_field(Entry::Show(show), field).await?)
}

/// `DELETE /admin/shows/<id>/seasons/<n>/locks/<field>`.
async fn unlock_season(
    State(catalogue): State<Catalogue>,
    Path((show, number, field)): Path<(i32, i32, String)>,
) -> Result<Json<Value>, ApiError> {
    let entry = Entry::Season { show, number };
    edited(catalogue.unlock_field(entry, field).await?)
}

/// `DELETE /admin/shows/<id>/seasons/<n>/episodes/<n>/locks/<field>`.
async fn unlock_episode(
    State(catalogue): State<Catalogue>,
    Path((show, season, number, field)): Path<(i32, i32, i32, String)>,
) -> Result<Json<Value>, ApiError> {
    let entry = Entry::Episode {
        show,
        season,
        number,
    };
    edited(catalogue.unlock_field(entry, field).await?)
}
