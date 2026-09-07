//! The append-only, hash-chained audit log.
//!
//! ## Why versioning isn't here (resolves 0003's open question)
//!
//! The plan doc asks whether temporal/SCD-2 versioning (`valid_from`/
//! `valid_to` on the memory row itself) applies to the embedded backend.
//! It doesn't, in this slice: `Memory` (see `crate::model`) is a single
//! current value, and `EngineBackend::put` overwrites in place (0003.2's
//! SQLite schema has no history columns). Full multi-version history was
//! always targeted at the production backend (decision-doc 0008 — Postgres
//! + pgvector + Apache AGE), not the embedded default.
//!
//! What z3rno's already-shipped product copy actually promises is narrower
//! and is what this module provides: "`forget` returns a proof of erasure,
//! and `audit` queries an append-only, hash-chained history of *what
//! changed and when*" — a durable log of operations, not a preserved copy
//! of every past value. That's exactly what an `AuditEvent` chain gives:
//! every `store`/`forget` appends one event, each hashing in the previous
//! event's hash, so the sequence can't be silently edited or reordered
//! without breaking the chain.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOperation {
    Store,
    Forget,
}

/// One entry in the hash chain. `hash` covers `(prev_hash, tenant_id,
/// operation, memory_id, at)` — verifying the chain means recomputing each
/// event's hash from those fields and checking it matches both the stored
/// `hash` and the next event's `prev_hash`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub tenant_id: String,
    pub operation: AuditOperation,
    pub memory_id: Uuid,
    pub at: DateTime<Utc>,
    pub prev_hash: Option<String>,
    pub hash: String,
}

impl AuditEvent {
    /// Builds the next event in a tenant's chain. `prev_hash` is the
    /// previous event's `hash`, or `None` for the first event.
    pub fn next(
        tenant_id: &str,
        operation: AuditOperation,
        memory_id: Uuid,
        prev_hash: Option<String>,
    ) -> Self {
        let id = Uuid::new_v4();
        let at = Utc::now();
        let hash = Self::compute_hash(&prev_hash, tenant_id, operation, memory_id, at);
        Self {
            id,
            tenant_id: tenant_id.to_string(),
            operation,
            memory_id,
            at,
            prev_hash,
            hash,
        }
    }

    fn compute_hash(
        prev_hash: &Option<String>,
        tenant_id: &str,
        operation: AuditOperation,
        memory_id: Uuid,
        at: DateTime<Utc>,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(prev_hash.as_deref().unwrap_or("genesis").as_bytes());
        hasher.update(b"|");
        hasher.update(tenant_id.as_bytes());
        hasher.update(b"|");
        hasher.update(format!("{operation:?}").as_bytes());
        hasher.update(b"|");
        hasher.update(memory_id.as_bytes());
        hasher.update(b"|");
        hasher.update(at.to_rfc3339().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Recomputes this event's hash from its own fields and checks it
    /// matches the stored `hash` — i.e. the event hasn't been tampered
    /// with in isolation (chain-level tampering — a swapped or deleted
    /// event — is caught by comparing consecutive `prev_hash`/`hash`
    /// pairs across the whole sequence, not by this method alone).
    pub fn is_self_consistent(&self) -> bool {
        Self::compute_hash(
            &self.prev_hash,
            &self.tenant_id,
            self.operation,
            self.memory_id,
            self.at,
        ) == self.hash
    }
}

/// Proof a `forget` call actually happened: the audit event it appended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForgetProof {
    pub audit_event_id: Uuid,
    pub hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_is_self_consistent_and_links_to_prev_hash() {
        let first = AuditEvent::next("tenant-a", AuditOperation::Store, Uuid::new_v4(), None);
        assert!(first.is_self_consistent());
        assert!(first.prev_hash.is_none());

        let second = AuditEvent::next(
            "tenant-a",
            AuditOperation::Forget,
            first.memory_id,
            Some(first.hash.clone()),
        );
        assert!(second.is_self_consistent());
        assert_eq!(second.prev_hash, Some(first.hash));
    }

    #[test]
    fn tampering_with_a_field_breaks_self_consistency() {
        let mut event = AuditEvent::next("tenant-a", AuditOperation::Store, Uuid::new_v4(), None);
        event.memory_id = Uuid::new_v4();
        assert!(!event.is_self_consistent());
    }
}
