//! Observability (0005.4): `tracing`-based logging, an optional
//! OpenTelemetry OTLP export bridge, and Prometheus metrics.
//!
//! `init_tracing()` covers the logging/tracing half: a plain
//! `tracing_subscriber::fmt` + env filter always works, and gets an OTel
//! layer on top only when both the `telemetry` Cargo feature is compiled
//! in *and* `OTEL_ENABLED=true` at runtime — see its doc comment for the
//! full matrix. `metrics` (this module's submodule) covers Prometheus:
//! `install_metrics_recorder`/`metrics_handler` for `/metrics`, and
//! `metrics_layer` as the per-request instrumentation, exposed here
//! because this crate's route-composition file (`lib.rs`) is out of this
//! sub-slice's scope — see that submodule's doc comment for how to wire
//! it in.

mod metrics;
pub use metrics::{install_metrics_recorder, metrics_handler, metrics_layer};

#[cfg(feature = "telemetry")]
mod otel;

/// Initializes the process-wide `tracing` subscriber. Call once, at
/// startup (idempotent in practice: a second call is a silent no-op
/// rather than a panic, which matters for tests that share a process).
///
/// - `telemetry` Cargo feature **off**: always the plain subscriber
///   below. The OTel crates aren't even in the dependency tree.
/// - `telemetry` **on**, `OTEL_ENABLED` unset or anything other than the
///   literal string `"true"`: still the plain subscriber — no exporter
///   is constructed, no network call is ever made. This is the
///   "compiled in but zero-overhead when disabled" requirement: carrying
///   the feature must not mean paying for it.
/// - `telemetry` **on**, `OTEL_ENABLED=true`: layers a real OTLP (gRPC)
///   exporter on top via `tracing-opentelemetry` — see `otel::init`.
pub fn init_tracing() {
    #[cfg(feature = "telemetry")]
    {
        if otel::enabled() {
            otel::init();
            return;
        }
    }
    init_plain();
}

fn init_plain() {
    // try_init, not init: a second call in the same process (repeated
    // test runs, `#[tokio::test]`s sharing a binary) must not panic just
    // because a global subscriber is already installed.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

// `OTEL_ENABLED` is process-wide mutable state read by `init_tracing()`
// (this module's test below) and written by `otel::enabled_requires_
// exactly_the_string_true` (cargo test runs both in the same binary,
// concurrently by default). Without this lock the two race: the other
// test can leave `OTEL_ENABLED=true` visible mid-flight, routing
// `init_tracing()` into the OTLP branch, which panics building a tonic
// exporter outside a `#[tokio::test]` runtime.
#[cfg(all(test, feature = "telemetry"))]
pub(crate) static OTEL_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_tracing_is_idempotent() {
        #[cfg(feature = "telemetry")]
        let _guard = OTEL_ENV_LOCK.lock().unwrap();

        // Must not panic, with or without a prior subscriber in this
        // process (test binaries run many #[test] fns in one process).
        init_tracing();
        init_tracing();
    }
}
