//! Stub — filled in by the auth sub-slice (0005.2). Implements the actual
//! JWT decode / API-key bcrypt verification behind `AuthContext`'s
//! `FromRequestParts` impl in `super`.

use axum::http::request::Parts;

use super::AuthContext;
use crate::error::ApiError;
use crate::AppState;

pub async fn authenticate(_parts: &mut Parts, _state: &AppState) -> Result<AuthContext, ApiError> {
    Err(ApiError::Internal(anyhow::anyhow!(
        "auth::extractor::authenticate is not implemented yet"
    )))
}
