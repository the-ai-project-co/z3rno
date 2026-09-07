//! Observability (0005.4): OpenTelemetry tracing (feature-gated
//! `telemetry`, no-op unless `OTEL_ENABLED=true` even when compiled in —
//! see module doc once filled in) + Prometheus metrics at `/metrics`.
//! Stub — filled in by the observability sub-slice.

/// Initializes `tracing` (and the OTel bridge, if the `telemetry` feature
/// is compiled in and `OTEL_ENABLED=true`). Call once, at startup.
pub fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}
