//! z3rno-engine: the core memory engine (store/recall/forget/audit).
//!
//! This crate is currently a scaffold with no real functionality yet.

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
