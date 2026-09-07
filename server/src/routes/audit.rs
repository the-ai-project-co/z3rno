//! Stub — filled in by its owning sub-slice.
use axum::Router;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}
