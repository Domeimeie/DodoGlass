//! Admin API for single Media files: their state, including Failed and
//! Missing ones, which the read API does not list.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use super::ApiError;
use crate::catalogue::{Catalogue, MediaFileStatus};

pub fn routes() -> Router<Catalogue> {
    Router::new().route("/admin/media-files/{id}", get(show))
}

/// `GET /admin/media-files/<id>`: `{"id", "state", "error"}`.
async fn show(
    State(catalogue): State<Catalogue>,
    Path(id): Path<String>,
) -> Result<Json<MediaFileStatus>, ApiError> {
    catalogue
        .media_file_status(&id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no Media file {id}")))
}
