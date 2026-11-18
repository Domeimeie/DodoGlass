//! Admin API for Unassigned media files.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::corrections::failure;
use crate::catalogue::{AttachTo, Attached, Catalogue, UnassignedView};
use crate::http::{ApiError, ApiState};
use crate::matcher::{AttachNew, Matcher};

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

/// Where to attach: an existing Movie or Episode, or a new one from TMDB.
#[derive(Deserialize)]
#[serde(untagged)]
enum AttachBody {
    Existing(AttachTo),
    New(AttachNew),
}

/// `POST /admin/unassigned-media-files/{id}/attach` with
/// `{"movie": {"name", "year"}}` or
/// `{"episode": {"show", "year", "season", "number"}}` (names and years of the
/// upload folders) to attach it to that existing Movie or Episode, or with
/// `{"new_movie": {"tmdb_id"}}` or
/// `{"new_episode": {"tmdb_id", "season", "number"}}` to attach it to a new
/// Movie, or an Episode of a new Show, created from TMDB with that Match.
/// Answers `{"id", "label"}`; 404 if the Media file is not Unassigned or the
/// Movie or Episode does not exist, 400 if TMDB does not know the id, 503 if
/// TMDB is unavailable.
async fn attach(
    State(catalogue): State<Catalogue>,
    State(matcher): State<Matcher>,
    Path(id): Path<String>,
    Json(to): Json<AttachBody>,
) -> Result<Json<Value>, ApiError> {
    let attached = match to {
        AttachBody::Existing(to) => catalogue.attach(&id, to).await?,
        AttachBody::New(to) => matcher.attach_new(&id, to).await?.map_err(failure)?,
    };
    match attached {
        Attached::Attached { label } => Ok(Json(json!({ "id": id, "label": label }))),
        Attached::NoSuchMediaFile => Err(ApiError::not_found("no such unassigned media file")),
        Attached::NoSuchTarget => Err(ApiError::not_found("no such movie or episode")),
    }
}
