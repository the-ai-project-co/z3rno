//! Storage interface layer: the three trait definitions every backend —
//! embedded or production — implements. The domain layer (memory tiers,
//! four-verb API) depends only on these traits, never on a concrete backend.

mod error;
mod graph;
mod relational;
mod vector;

pub mod embedded;

#[cfg(any(test, feature = "testing"))]
pub mod mock;

pub use error::{BackendError, BackendResult};
pub use graph::{GraphBackend, GraphEdge};
pub use relational::{EngineBackend, Record};
pub use vector::{VectorBackend, VectorMatch};

pub use embedded::graph::EmbeddedGraphBackend;
pub use embedded::sqlite::SqliteEngineBackend;
pub use embedded::vector::EmbeddedVectorBackend;
