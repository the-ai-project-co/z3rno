//! z3rno-server: an Axum HTTP API in front of `z3rno-engine`, for shared,
//! production, and multi-tenant deployments (slice 0005). The engine
//! itself already runs embedded with zero infrastructure — this crate is
//! what you reach for when you need it reachable over the network,
//! authenticated, and shared across processes or languages.
//!
//! Structured as a library + a thin `[[bin]]` (see Cargo.toml's doc
//! comment) so `build_router` is testable via `tower::ServiceExt::oneshot`
//! without binding a real socket — the pattern `_research_refs/cognee-rs`'s
//! own http-server crate documents and uses.

pub mod auth;
pub mod cache;
pub mod error;
pub mod observability;
pub mod routes;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use z3rno_engine::MemoryEngine;

use crate::auth::AuthConfig;
use crate::cache::CacheBackend;

/// Shared state every route handler gets via `axum::extract::State`.
/// `Clone` is cheap — every field is an `Arc` (or `Arc`-wrapping trait
/// object), matching cognee-rs's documented `AppState` shape.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<MemoryEngine>,
    pub cache: Arc<dyn CacheBackend>,
    pub auth: Arc<AuthConfig>,
}

/// A generous but real cap (16 MiB) — the old system's `body-limit`
/// middleware step, preserved so a request can't exhaust memory before
/// auth/routing ever see it. Not configurable yet; revisit if a real
/// deployment needs a different ceiling.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Builds the full router: every route module merged in, plus the
/// cross-cutting layers (CORS, request tracing, body-size limit). Per-
/// route auth/RBAC is applied inside each route module, not here — the
/// old system's RBAC was a per-route dependency, not a blanket gate,
/// since `/health` and friends must stay reachable unauthenticated.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(routes::memories::router())
        .merge(routes::audit::router())
        .merge(routes::sessions::router())
        .merge(routes::admin::router())
        .merge(routes::health::router())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Binds `addr` and serves `build_router(state)` until the process is
/// asked to stop. The `[[bin]]` target is the only caller in this repo;
/// tests build the router directly and drive it with `oneshot` instead.
pub async fn run(addr: SocketAddr, state: AppState) -> anyhow::Result<()> {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "z3rno-server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
