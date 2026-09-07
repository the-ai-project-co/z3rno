//! The OTLP (gRPC) tracing bridge. Only compiled with the `telemetry`
//! Cargo feature; only exercised at runtime when `OTEL_ENABLED=true` —
//! see `super::init_tracing`'s doc comment for the full matrix this file
//! is one branch of.
//!
//! No concrete pattern was found to follow here: this repo's
//! `_research_refs` directory (cited in other modules' doc comments as
//! holding a reference implementation) doesn't exist in this worktree —
//! there's no vendored reference crate to check against, so this is a
//! plain reading of `opentelemetry-otlp`/`tracing-opentelemetry`'s own
//! documented usage.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// `OTEL_ENABLED` must be exactly `"true"` — anything else (unset, `"1"`,
/// `"TRUE"`, ...) keeps this whole bridge dark, matching the "opt in
/// deliberately" requirement.
pub(super) fn enabled() -> bool {
    std::env::var("OTEL_ENABLED").as_deref() == Ok("true")
}

/// Builds the OTLP exporter, layers `tracing-opentelemetry` on top of the
/// same `fmt` + env-filter stack `super::init_plain` uses, and installs
/// it as the process's global subscriber.
///
/// The collector endpoint is read by `opentelemetry-otlp` itself from
/// `OTEL_EXPORTER_OTLP_ENDPOINT` (its own documented default is
/// `http://localhost:4317` when that's unset) — not reimplemented here,
/// per the sub-slice brief.
pub(super) fn init() {
    match build_tracer() {
        Ok(tracer) => {
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            let _ = tracing_subscriber::registry()
                .with(tracing_subscriber::EnvFilter::from_default_env())
                .with(tracing_subscriber::fmt::layer())
                .with(otel_layer)
                .try_init();
        }
        Err(err) => {
            // A broken/unreachable collector shouldn't take logging down
            // with it — fall back to the plain subscriber instead of
            // leaving the process with none at all.
            eprintln!(
                "z3rno-server: failed to initialize the OTLP exporter ({err}), \
                 falling back to plain tracing"
            );
            super::init_plain();
        }
    }
}

fn build_tracer() -> anyhow::Result<opentelemetry_sdk::trace::Tracer> {
    use opentelemetry::trace::TracerProvider as _;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;
    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();
    let tracer = provider.tracer("z3rno-server");
    opentelemetry::global::set_tracer_provider(provider);
    Ok(tracer)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only `enabled()`'s parsing is unit-testable in isolation; `init()`
    // requires a real collector to verify anything beyond "did it
    // panic", which `observability::tests::init_tracing_is_idempotent`
    // (in the parent module, with OTEL_ENABLED left unset) already
    // covers for the disabled path.
    #[test]
    fn enabled_requires_exactly_the_string_true() {
        std::env::set_var("OTEL_ENABLED", "true");
        assert!(enabled());

        std::env::set_var("OTEL_ENABLED", "TRUE");
        assert!(!enabled());

        std::env::set_var("OTEL_ENABLED", "1");
        assert!(!enabled());

        std::env::remove_var("OTEL_ENABLED");
        assert!(!enabled());
    }
}
