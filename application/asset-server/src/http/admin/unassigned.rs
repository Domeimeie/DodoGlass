//! Admin API for Unassigned media files.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::catalogue::{AttachTo, Attached, Catalogue, UnassignedView};
use crate::http::{ApiError, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/unassigned-media-files", get(list))
        .route("/unassigned-media-files/{id}/attach", post(attach))
}

/// `GET /admin/unassigned-media-files`: every Unassigned media file, with its
/// upload path, the reason it did not fit and its state.
async fn list(State(catalogue): State<Catalogue>) -> Result<Json<Vec<UnassignedView>>, ApiError> {
    Ok(Json(catalogue.unassigned_media_files().await?))
}

/// `POST /admin/unassigned-media-files/{id}/attach` with
/// `{"movie": {"name", "year"}}` or
/// `{"episode": {"show", "year", "season", "number"}}` (names and years of the
/// upload folders): attaches it to that existing Movie or Episode. Answers
/// `{"id", "label"}`; 404 if the Media file is not Unassigned or the Movie or
/// Episode does not exist.
async fn attach(
    State(catalogue): State<Catalogue>,
    Path(id): Path<String>,
    Json(to): Json<AttachTo>,
) -> Result<Json<Value>, ApiError> {
    match catalogue.attach(&id, to).await? {
        Attached::Attached { label } => Ok(Json(json!({ "id": id, "label": label }))),
        Attached::NoSuchMediaFile => Err(ApiError::not_found("no such unassigned media file")),
        Attached::NoSuchTarget => Err(ApiError::not_found("no such movie or episode")),
    }
}
