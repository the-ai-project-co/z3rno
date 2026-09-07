//! z3rno-engine: the core memory engine (store/recall/forget/audit).

pub mod audit;
pub mod backend;
pub mod engine;
pub mod model;

pub use audit::{AuditEvent, AuditOperation, ForgetProof};
pub use engine::{Advanced, MemoryEngine};
pub use model::{Memory, Tier};

/// Returns the crate's version, as set in `Cargo.toml`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
}
