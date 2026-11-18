//! The admin API, served under `/admin`. No authentication: it is reachable
//! from localhost only until the User service exists.
//!
//! Each part of the admin API is its own module with its own `routes()`
//! (paths relative to `/admin`), merged here.

mod media_files;
mod review;
mod unassigned;

use axum::Router;

use crate::catalogue::Catalogue;

pub fn routes() -> Router<Catalogue> {
    Router::new()
        .merge(media_files::routes())
        .merge(review::routes())
        .merge(unassigned::routes())
}
