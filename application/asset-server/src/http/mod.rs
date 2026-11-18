//! The HTTP API (axum). Handlers are thin: they call the Catalogue and turn
//! its answer into JSON. Each part of the API is its own module with its own
//! `routes()`, merged here.

mod error;
mod media_files;
mod movies;

use axum::Router;

use crate::catalogue::Catalogue;

pub use error::ApiError;

pub fn router(catalogue: Catalogue) -> Router {
    Router::new()
        .merge(movies::routes())
        .merge(media_files::routes())
        .with_state(catalogue)
}
