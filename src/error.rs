use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum LificError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Too many requests: {0}")]
    TooManyRequests(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntoResponse for LificError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            LificError::Database(e) => {
                // Log the real error server-side, return generic message to client
                error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
            LificError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            LificError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            LificError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            LificError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            LificError::TooManyRequests(msg) => (StatusCode::TOO_MANY_REQUESTS, msg.clone()),
            LificError::Internal(msg) => {
                error!(error = %msg, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };

        let body = json!({ "error": message });
        let mut response = (status, axum::Json(body)).into_response();
        if matches!(self, LificError::TooManyRequests(_)) {
            // Export slots are typically freed in seconds, but a stalled
            // stream holds one until the 30-second idle timeout reaps it.
            // Advertise that horizon rather than an optimistic 1 (LIF-424).
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("30"),
            );
        }
        response
    }
}
