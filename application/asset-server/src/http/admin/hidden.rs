//! Admin API: deleting Hidden Movies, Shows, Seasons and Episodes for good.
//! The paths follow the read API's; an entry that is not Hidden is refused
//! (409).

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::delete;

use super::media_files::answer;
use crate::catalogue::{Catalogue, Entry};
use crate::http::ApiError;
use crate::http::ApiState;

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/movies/{movie}", delete(delete_movie))
        .route("/shows/{show}", delete(delete_show))
        .route("/shows/{show}/seasons/{season}", delete(delete_season))
        .route(
            "/shows/{show}/seasons/{season}/episodes/{episode}",
            delete(delete_episode),
        )
}

async fn delete_entry(catalogue: Catalogue, entry: Entry) -> Result<StatusCode, ApiError> {
    answer(catalogue.delete_hidden(entry).await?, "such entry")
}

/// `DELETE /admin/movies/<id>`: with its metadata and Media files.
async fn delete_movie(
    State(catalogue): State<Catalogue>,
    Path(movie): Path<i32>,
) -> Result<StatusCode, ApiError> {
    delete_entry(catalogue, Entry::Movie(movie)).await
}

/// `DELETE /admin/shows/<id>`: with its Seasons, Episodes and Media files.
async fn delete_show(
    State(catalogue): State<Catalogue>,
    Path(show): Path<i32>,
) -> Result<StatusCode, ApiError> {
    delete_entry(catalogue, Entry::Show(show)).await
}

/// `DELETE /admin/shows/<id>/seasons/<number>`: with its Episodes and Media
/// files.
async fn delete_season(
    State(catalogue): State<Catalogue>,
    Path((show, number)): Path<(i32, i32)>,
) -> Result<StatusCode, ApiError> {
    delete_entry(catalogue, Entry::Season { show, number }).await
}

/// `DELETE /admin/shows/<id>/seasons/<number>/episodes/<number>`: with its
/// Media files.
async fn delete_episode(
    State(catalogue): State<Catalogue>,
    Path((show, season, number)): Path<(i32, i32, i32)>,
) -> Result<StatusCode, ApiError> {
    let entry = Entry::Episode {
        show,
        season,
        number,
    };
    delete_entry(catalogue, entry).await
}
