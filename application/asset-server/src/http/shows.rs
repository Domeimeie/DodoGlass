//! Read API for Shows, their Seasons and their Episodes.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use super::ApiError;
use super::ApiState;
use crate::catalogue::{Catalogue, EpisodeView, SeasonView, ShowView};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/shows", get(list))
        .route("/shows/{show}/seasons", get(seasons))
        .route("/shows/{show}/seasons/{season}/episodes", get(episodes))
}

/// `GET /shows`: the Shows that are not Hidden.
async fn list(State(catalogue): State<Catalogue>) -> Result<Json<Vec<ShowView>>, ApiError> {
    Ok(Json(catalogue.visible_shows().await?))
}

/// `GET /shows/{show}/seasons`: the Seasons of a Show that are not Hidden;
/// 404 if the Show is unknown or Hidden.
async fn seasons(
    State(catalogue): State<Catalogue>,
    Path(show): Path<i32>,
) -> Result<Json<Vec<SeasonView>>, ApiError> {
    catalogue
        .visible_seasons(show)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no such show"))
}

/// `GET /shows/{show}/seasons/{season}/episodes`: the Episodes of a Season
/// that are not Hidden, with their Ready Media files; 404 if the Season is
/// unknown or Hidden.
async fn episodes(
    State(catalogue): State<Catalogue>,
    Path((show, season)): Path<(i32, i32)>,
) -> Result<Json<Vec<EpisodeView>>, ApiError> {
    catalogue
        .visible_episodes(show, season)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no such season"))
}
