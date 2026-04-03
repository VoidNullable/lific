use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum LificError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntoResponse for LificError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            LificError::Database(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            LificError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            LificError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            LificError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
        };

        let body = json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}
