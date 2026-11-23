//! One error envelope for the whole server: a single `ApiError`
//! implementing `IntoResponse`. The health router is the one deliberate
//! exception — it returns its own bespoke
//! `{status, reason}` shape instead of this envelope, matching the old
//! Python API's health response verbatim rather than "fixing" it into
//! consistency it never had.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use z3rno_engine::backend::{BackendError, ErrorCode};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// The operation partly happened and is safe to retry (e.g. a forget
    /// whose derived-store cleanup is still pending).
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m.clone()),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m.clone()),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            ApiError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m.clone()),
            ApiError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

impl From<BackendError> for ApiError {
    fn from(err: BackendError) -> Self {
        match err {
            BackendError::NotFound(m) => ApiError::NotFound(m),
            BackendError::Rejected {
                code: ErrorCode::NotPermitted,
                message,
            } => ApiError::Forbidden(message),
            err @ BackendError::Rejected { .. } => ApiError::BadRequest(err.to_string()),
            err @ BackendError::ForgetIncomplete { .. } => ApiError::Unavailable(err.to_string()),
            BackendError::Storage(e) => ApiError::Internal(e),
        }
    }
}
