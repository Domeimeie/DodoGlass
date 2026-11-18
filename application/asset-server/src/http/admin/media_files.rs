//! Admin API for single Media files: their state, including Failed and
//! Missing ones, which the read API does not list, and the admin's changes
//! to them.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;

use crate::catalogue::{Catalogue, Change, MediaFileStatus};
use crate::http::ApiError;

pub fn routes() -> Router<Catalogue> {
    Router::new()
        .route("/media-files/{id}", get(show).patch(edit))
        .route("/media-files/{id}/default", put(set_default))
}

/// A [`Change`] as an answer: 204, 404, 409 with the rule it breaks, or 400.
pub(super) fn answer(change: Change, what: &str) -> Result<StatusCode, ApiError> {
    match change {
        Change::Done => Ok(StatusCode::NO_CONTENT),
        Change::NotFound => Err(ApiError::not_found(format!("no {what}"))),
        Change::Refused(message) => Err(ApiError::conflict(message)),
        Change::Invalid(message) => Err(ApiError::bad_request(message)),
    }
}

#[derive(Deserialize)]
struct Edit {
    label: String,
}

/// `PATCH /admin/media-files/<id>` with `{"label"}`: renames its Label. 409
/// if another Media file of the same Movie or Episode has it, 400 if empty.
async fn edit(
    State(catalogue): State<Catalogue>,
    Path(id): Path<String>,
    Json(edit): Json<Edit>,
) -> Result<StatusCode, ApiError> {
    answer(
        catalogue.rename_label(&id, &edit.label).await?,
        "such Media file",
    )
}

/// `GET /admin/media-files/<id>`: `{"id", "key", "label", "state", "error"}`.
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

/// `PUT /admin/media-files/<id>/default`: makes this Ready Media file the
/// Default media file of its Movie or Episode. 409 if it is not Ready or is
/// Unassigned.
async fn set_default(
    State(catalogue): State<Catalogue>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    answer(
        catalogue.set_default_media_file(&id).await?,
        "such Media file",
    )
}
