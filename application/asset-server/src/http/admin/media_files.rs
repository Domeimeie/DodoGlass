//! Admin API for single Media files: their state, including Failed and
//! Missing ones, which the read API does not list, and the admin's changes
//! to them.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;

use crate::catalogue::{Catalogue, Change, MediaFileState, MediaFileStatus};
use crate::http::ApiError;

pub fn routes() -> Router<Catalogue> {
    Router::new()
        .route("/media-files", get(list))
        .route("/media-files/{id}", get(show).patch(edit).delete(remove))
        .route("/media-files/{id}/default", put(set_default))
}

#[derive(Deserialize)]
struct ListQuery {
    /// Comma-separated states (`failed,missing`); every state if absent.
    state: Option<String>,
}

/// `GET /admin/media-files?state=failed,missing`: the Media files in those
/// states, by upload key, each as `GET /admin/media-files/<id>` answers (the
/// error of a Failed one, the key of a Missing one). 400 for an unknown state.
async fn list(
    State(catalogue): State<Catalogue>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<MediaFileStatus>>, ApiError> {
    let states = match query.state {
        None => vec![
            MediaFileState::Processing,
            MediaFileState::Ready,
            MediaFileState::Failed,
            MediaFileState::Missing,
        ],
        Some(states) => states
            .split(',')
            .map(|s| MediaFileState::parse(s.trim()))
            .collect::<Result<_, _>>()
            .map_err(|e| ApiError::bad_request(e.to_string()))?,
    };
    Ok(Json(catalogue.media_files_in(states).await?))
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

/// `DELETE /admin/media-files/<id>`: deletes its output in `final` and its
/// entry; its Movie or Episode stays.
async fn remove(
    State(catalogue): State<Catalogue>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    answer(catalogue.remove_media_file(&id).await?, "such Media file")
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
