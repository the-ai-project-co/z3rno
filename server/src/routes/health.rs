//! `/v1/health`, `/v1/health/detailed`, `/metrics`.
//!
//! Health is a deliberate exception to the rest of the server's error
//! handling (see `crate::error`'s module doc): its own bespoke response
//! shape, never `ApiError`/`ApiResult`. It's also, deliberately, not the
//! old Python API's `/v1/ready` — that handler was a confirmed no-op bug,
//! unconditionally returning `status="ok", database="connected",
//! redis="connected"` with no check ever run. Both endpoints here run
//! two *real* checks — a query against whichever engine backend is live,
//! and a read against the cache backend — and only differ in how much
//! detail the response exposes:
//!
//! - `/v1/health`: cheap, for a load-balancer/k8s liveness probe. Same
//!   checks, just the aggregate status.
//! - `/v1/health/detailed`: the same checks, broken out per component.
//!
//! Each check runs in its own `tokio::spawn` so a panic inside one can't
//! take the handler down with it — a health check must never itself
//! become the thing that crashes the server.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use z3rno_engine::{BackendTier, MemoryEngine};

use crate::cache::CacheBackend;
use crate::observability;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/health/detailed", get(health_detailed))
        .route("/metrics", get(observability::metrics_handler))
}

/// Neither a tenant nor a memory that's ever expected to exist — just a
/// fixed key/id to query against, so the check exercises a real
/// round-trip through the live backend without depending on any real
/// data being present. An empty/miss result is the expected *healthy*
/// outcome; only an error or a timeout means the backend didn't answer.
const HEALTH_CHECK_KEY: &str = "__z3rno_health_check__";

/// How long a single component check is allowed to take before it counts
/// as unhealthy. Generous enough for a real network round-trip
/// (Postgres, Redis), tight enough that a liveness probe doesn't hang.
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// Binary on purpose: both checks here either fully succeed or they
/// don't — there's no partial-success state for "did a query return"
/// worth inventing a `Degraded` variant for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ComponentStatus {
    Healthy,
    Unhealthy,
}

impl ComponentStatus {
    fn is_healthy(self) -> bool {
        self == ComponentStatus::Healthy
    }
}

fn overall(components: &BTreeMap<String, ComponentStatus>) -> ComponentStatus {
    if components.values().all(|s| s.is_healthy()) {
        ComponentStatus::Healthy
    } else {
        ComponentStatus::Unhealthy
    }
}

fn status_code(status: ComponentStatus) -> StatusCode {
    match status {
        ComponentStatus::Healthy => StatusCode::OK,
        ComponentStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// A genuine query against whichever backend is live: SQLite directly for
/// the embedded tier, a real round-trip through the connection pool for
/// Postgres. Isolated in its own task so a panic here can't propagate
/// into the health handler.
async fn check_engine(engine: Arc<MemoryEngine>) -> ComponentStatus {
    let task = tokio::spawn(async move {
        tokio::time::timeout(
            HEALTH_CHECK_TIMEOUT,
            engine.advanced().audit(HEALTH_CHECK_KEY),
        )
        .await
    });
    match task.await {
        Ok(Ok(Ok(_))) => ComponentStatus::Healthy,
        // Backend error, timeout elapsed, or the spawned task panicked
        // (JoinError) — all three mean "didn't get a real answer".
        _ => ComponentStatus::Unhealthy,
    }
}

/// Same pattern against the cache backend — SQLite or Redis depending on
/// what's configured.
async fn check_cache(cache: Arc<dyn CacheBackend>) -> ComponentStatus {
    let task = tokio::spawn(async move {
        tokio::time::timeout(HEALTH_CHECK_TIMEOUT, cache.get(HEALTH_CHECK_KEY)).await
    });
    match task.await {
        Ok(Ok(Ok(_))) => ComponentStatus::Healthy,
        _ => ComponentStatus::Unhealthy,
    }
}

fn engine_component_name(engine: &MemoryEngine) -> &'static str {
    match engine.tier() {
        BackendTier::Embedded => "engine (embedded)",
        BackendTier::Postgres => "engine (postgres)",
    }
}

async fn run_checks(state: &AppState) -> BTreeMap<String, ComponentStatus> {
    let engine_name = engine_component_name(&state.engine);
    let (engine_status, cache_status) = tokio::join!(
        check_engine(state.engine.clone()),
        check_cache(state.cache.clone()),
    );

    let mut components = BTreeMap::new();
    components.insert(engine_name.to_string(), engine_status);
    components.insert("cache".to_string(), cache_status);
    components
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: ComponentStatus,
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let components = run_checks(&state).await;
    let status = overall(&components);
    (status_code(status), Json(HealthResponse { status }))
}

#[derive(Debug, Serialize)]
struct DetailedHealthResponse {
    status: ComponentStatus,
    components: BTreeMap<String, ComponentStatus>,
}

async fn health_detailed(
    State(state): State<AppState>,
) -> (StatusCode, Json<DetailedHealthResponse>) {
    let components = run_checks(&state).await;
    let status = overall(&components);
    (
        status_code(status),
        Json(DetailedHealthResponse { status, components }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthConfig;
    use crate::cache::SqliteCacheBackend;

    fn test_state() -> AppState {
        let engine =
            MemoryEngine::embedded(tempfile::NamedTempFile::new().unwrap().path()).unwrap();
        let cache = SqliteCacheBackend::open_in_memory().unwrap();
        AppState {
            engine: Arc::new(engine),
            cache: Arc::new(cache),
            auth: Arc::new(AuthConfig {
                jwt_secret: "test".into(),
                superadmin_api_key: None,
            }),
        }
    }

    #[tokio::test]
    async fn health_reports_healthy_against_a_working_backend() {
        let state = test_state();
        let (status, Json(body)) = health(State(state)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.status, ComponentStatus::Healthy);
    }

    #[tokio::test]
    async fn health_detailed_breaks_out_each_component_healthy() {
        let state = test_state();
        let (status, Json(body)) = health_detailed(State(state)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.status, ComponentStatus::Healthy);
        assert_eq!(body.components.len(), 2);
        assert!(body.components.contains_key("engine (embedded)"));
        assert!(body.components.contains_key("cache"));
        for component_status in body.components.values() {
            assert_eq!(*component_status, ComponentStatus::Healthy);
        }
    }

    // Status-aggregation logic, covering the unhealthy path without
    // standing up a broken database (per the sub-slice brief: a unit
    // test on the aggregation given fake component results is a
    // legitimate substitute for exhaustively breaking a real backend).
    #[test]
    fn overall_is_unhealthy_if_any_component_is_unhealthy() {
        let mut components = BTreeMap::new();
        components.insert("engine (embedded)".to_string(), ComponentStatus::Healthy);
        components.insert("cache".to_string(), ComponentStatus::Unhealthy);

        assert_eq!(overall(&components), ComponentStatus::Unhealthy);
        assert_eq!(
            status_code(overall(&components)),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn overall_is_healthy_only_if_every_component_is() {
        let mut components = BTreeMap::new();
        components.insert("engine (embedded)".to_string(), ComponentStatus::Healthy);
        components.insert("cache".to_string(), ComponentStatus::Healthy);

        assert_eq!(overall(&components), ComponentStatus::Healthy);
        assert_eq!(status_code(overall(&components)), StatusCode::OK);
    }
}
