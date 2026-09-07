//! Prometheus metrics (0005.4 part B): a global recorder installed once
//! via `PrometheusBuilder`, a `/metrics` handler that renders it, and a
//! request counter + latency histogram recorded per method+path+status.
//!
//! **Integration note**: `metrics_layer` is written as an
//! `axum::middleware::from_fn`-compatible function rather than applied
//! anywhere in this crate, because the only place to add a top-level
//! layer is `lib.rs`'s `build_router` — out of this sub-slice's scope
//! (see that file's module doc). Wire it in with:
//!
//! ```ignore
//! .route_layer(axum::middleware::from_fn(observability::metrics_layer))
//! ```
//!
//! `route_layer` (not `layer`) matters: it runs *after* route matching,
//! so the `MatchedPath` extension this function reads is already
//! populated and the `path` label stays one series per route template
//! (e.g. `/v1/sessions/{id}`) instead of one per literal request path.

use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static RECORDER: OnceLock<PrometheusHandle> = OnceLock::new();

/// Installs the global Prometheus recorder on first call; every later
/// call (from the `/metrics` handler, the metrics middleware, or a test)
/// just returns a clone of the same handle. Safe to call from more than
/// one place — that's the point, since callers don't coordinate startup
/// order with each other.
pub fn install_metrics_recorder() -> PrometheusHandle {
    RECORDER
        .get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("failed to install the global Prometheus recorder")
        })
        .clone()
}

/// `GET /metrics` — the current snapshot in the Prometheus text
/// exposition format.
pub async fn metrics_handler() -> impl IntoResponse {
    let handle = install_metrics_recorder();
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        handle.render(),
    )
}

/// Records `http_requests_total` (a counter) and
/// `http_request_duration_seconds` (a histogram), both labeled by
/// `method`, `path`, and `status`, for every request that reaches it.
pub async fn metrics_layer(
    matched_path: Option<MatchedPath>,
    req: Request,
    next: Next,
) -> Response {
    install_metrics_recorder();

    let method = req.method().to_string();
    // Falls back to the raw URI path if this ran before route matching
    // (i.e. wired with `.layer()` instead of the recommended
    // `.route_layer()`) — still correct, just higher-cardinality for any
    // route with a path parameter.
    let path = matched_path
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

    let start = Instant::now();
    let response = next.run(req).await;
    let elapsed = start.elapsed();
    let status = response.status().as_u16().to_string();

    metrics::counter!(
        "http_requests_total",
        "method" => method.clone(),
        "path" => path.clone(),
        "status" => status.clone(),
    )
    .increment(1);
    metrics::histogram!(
        "http_request_duration_seconds",
        "method" => method,
        "path" => path,
        "status" => status,
    )
    .record(elapsed.as_secs_f64());

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_recorded_metric() {
        install_metrics_recorder();
        metrics::counter!("z3rno_test_counter_total").increment(1);
        let handle = install_metrics_recorder();
        let rendered = handle.render();
        assert!(!rendered.is_empty());
        assert!(rendered.contains("z3rno_test_counter_total"));
    }
}
