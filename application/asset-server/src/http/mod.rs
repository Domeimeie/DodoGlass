//! The HTTP API (axum). Handlers are thin: they call the Catalogue (or, for
//! what needs TMDB, the Matcher) and turn its answer into JSON. Each part of
//! the API is its own module with its own `routes()`, merged here. The admin
//! API is nested under `/admin` (see [`admin`]).

mod admin;
mod error;
pub mod images;
mod movies;
mod shows;

use axum::Router;
use axum::extract::FromRef;

use crate::catalogue::Catalogue;
use crate::matcher::Matcher;

pub use error::ApiError;

/// What handlers can take as `State<Catalogue>` or `State<Matcher>`.
#[derive(Clone)]
pub struct ApiState {
    pub catalogue: Catalogue,
    pub matcher: Matcher,
}

impl FromRef<ApiState> for Catalogue {
    fn from_ref(state: &ApiState) -> Self {
        state.catalogue.clone()
    }
}

impl FromRef<ApiState> for Matcher {
    fn from_ref(state: &ApiState) -> Self {
        state.matcher.clone()
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .merge(movies::routes())
        .merge(shows::routes())
        .nest("/admin", admin::routes())
        .with_state(state)
}
