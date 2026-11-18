//! Read API for Movies.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use super::ApiError;
use crate::catalogue::{Catalogue, MovieView};

pub fn routes() -> Router<Catalogue> {
    Router::new().route("/movies", get(list))
}

/// `GET /movies`: the Movies that are not Hidden.
async fn list(State(catalogue): State<Catalogue>) -> Result<Json<Vec<MovieView>>, ApiError> {
    Ok(Json(catalogue.visible_movies().await?))
}
