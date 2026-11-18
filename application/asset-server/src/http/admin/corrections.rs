//! Admin API: metadata corrections. Picking a Match for a Movie or Show (one
//! of its candidates, or any TMDB id).
//!
//! TMDB refusing (no such id) is a 400, TMDB being unavailable a 503.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::put;
use axum::{Json, Router};
use serde::Deserialize;

use crate::http::{ApiError, ApiState};
use crate::matcher::{Corrected, Matcher};

pub fn routes() -> Router<ApiState> {
    Router::new().route("/movies/{movie}/match", put(pick_movie_match))
}

/// A [`Corrected`] as an answer: 204, 404, 400 or 503.
fn answer(corrected: Corrected) -> Result<StatusCode, ApiError> {
    match corrected {
        Corrected::Done => Ok(StatusCode::NO_CONTENT),
        Corrected::NotFound(message) => Err(ApiError::not_found(message)),
        Corrected::Rejected(message) => Err(ApiError::bad_request(message)),
        Corrected::Unavailable(message) => Err(ApiError::unavailable(message)),
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
