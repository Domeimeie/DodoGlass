use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tracing::error;

/// An error answered as `{"error": "..."}` with its status.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn not_found(message: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

/// Anything unexpected (database, storage) is a 500.
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        error!(error = format!("{e:#}"), "request failed");
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal error".into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
