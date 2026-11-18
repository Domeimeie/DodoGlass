//! Admin API: the Needs review list. Picking a Match comes with the other
//! admin corrections.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::catalogue::{Catalogue, NeedsReviewMovie};
use crate::http::ApiError;

pub fn routes() -> Router<Catalogue> {
    Router::new().route("/needs-review", get(list))
}

#[derive(Serialize)]
struct NeedsReview {
    movies: Vec<NeedsReviewMovie>,
}

/// `GET /admin/needs-review`: every entry in Needs review with its candidates.
async fn list(State(catalogue): State<Catalogue>) -> Result<Json<NeedsReview>, ApiError> {
    Ok(Json(NeedsReview {
        movies: catalogue.movies_needing_review().await?,
    }))
}
