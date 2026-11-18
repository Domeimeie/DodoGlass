//! The admin API, served under `/admin`. No authentication: it is reachable
//! from localhost only until the User service exists.
//!
//! Each part of the admin API is its own module with its own `routes()`
//! (paths relative to `/admin`), merged here.

mod corrections;
mod hidden;
mod media_files;
mod review;
mod unassigned;

use axum::Router;

use crate::http::ApiState;

pub fn routes() -> Router<ApiState> {
    Router::new()
        .merge(corrections::routes())
        .merge(hidden::routes())
        .merge(media_files::routes())
        .merge(review::routes())
        .merge(unassigned::routes())
}
