//! The production backend: Postgres + pgvector + Apache AGE, selected in
//! `_decision_docs/0008-production-backend-selection.md`. Implements the
//! same `EngineBackend`/`VectorBackend`/`GraphBackend` traits the embedded
//! backend does — the engine, server, and bindings don't need to know or
//! care which is active.

mod graph;
mod provision;
mod relational;
mod vector;

pub use graph::PostgresGraphBackend;
pub use provision::{connect, ensure_extensions, BOOTSTRAP_SQL};
pub use relational::PostgresEngineBackend;
pub use vector::PostgresVectorBackend;
