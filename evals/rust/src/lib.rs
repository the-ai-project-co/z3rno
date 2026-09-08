//! Pure, independently-testable pieces of the Rust golden-dataset eval
//! harness (slice 0008.4). The actual end-to-end run against a live
//! `MemoryEngine` lives in `tests/golden_run.rs` — this crate only holds
//! the scoring math and the fixture loader, so they can be unit-tested in
//! isolation from the engine.

pub mod dataset;
pub mod metrics;

pub use dataset::{Dataset, Item, Seed};
pub use metrics::{faithfulness, latency_percentiles, mrr, recall_at_k, LatencyPercentiles};
