//! Stub — filled in by its owning sub-slice.
use crate::AppState;
use axum::Router;

pub fn router() -> Router<AppState> {
    Router::new()
}
