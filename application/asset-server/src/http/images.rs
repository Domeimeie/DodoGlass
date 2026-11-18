//! `GET /images/<key>`: streams an image (a poster) from the `assets` bucket.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UriPath, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::TryStreamExt;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};

use super::ApiError;

/// The image routes, with the `assets` bucket as their state.
pub fn router(assets: Arc<dyn ObjectStore>) -> Router {
    Router::new()
        .route("/images/{*key}", get(image))
        .with_state(assets)
}

async fn image(
    State(assets): State<Arc<dyn ObjectStore>>,
    UriPath(key): UriPath<String>,
) -> Result<Response, ApiError> {
    let not_found = || ApiError::not_found(format!("no image {key}"));
    let location = Path::parse(&key).map_err(|_| not_found())?;
    let found = match assets.get(&location).await {
        Ok(found) => found,
        Err(object_store::Error::NotFound { .. }) => return Err(not_found()),
        Err(e) => return Err(anyhow::Error::from(e).into()),
    };
    let length = found.meta.size;
    let body = Body::from_stream(found.into_stream().map_err(std::io::Error::other));
    Ok((
        [
            (header::CONTENT_TYPE, content_type(&key).to_string()),
            (header::CONTENT_LENGTH, length.to_string()),
            // A key never changes content: TMDB's file names are unique.
            (
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable".into(),
            ),
        ],
        body,
    )
        .into_response())
}

fn content_type(key: &str) -> &'static str {
    match key
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
    {
        Some(ext) if ext == "jpg" || ext == "jpeg" => "image/jpeg",
        Some(ext) if ext == "png" => "image/png",
        Some(ext) if ext == "webp" => "image/webp",
        Some(ext) if ext == "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}
