//! Admin API for Unassigned media files.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::catalogue::{Catalogue, UnassignedView};
use crate::http::ApiError;

pub fn routes() -> Router<Catalogue> {
    Router::new().route("/unassigned-media-files", get(list))
}

/// `GET /admin/unassigned-media-files`: every Unassigned media file, with its
/// upload path, the reason it did not fit and its state.
async fn list(State(catalogue): State<Catalogue>) -> Result<Json<Vec<UnassignedView>>, ApiError> {
    Ok(Json(catalogue.unassigned_media_files().await?))
}
