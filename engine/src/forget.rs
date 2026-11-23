//! The durable intent record behind `MemoryEngine::forget`.
//!
//! A memory lives in three stores (relational, vector, graph) that share no
//! transaction. `forget` therefore runs as a small state machine whose
//! progress is itself stored:
//!
//! 1. write a `forget_pending` tombstone for the memory id;
//! 2. delete the relational row, which hides the memory from every read
//!    path at once (recall, list, neighbors all join back through it);
//! 3. delete the vector entry and the graph node, collecting failures;
//! 4. only when both succeed, append the `Forget` audit event and remove
//!    the tombstone.
//!
//! A crash or error anywhere after step 1 leaves the tombstone behind, so a
//! later `forget` of the same id (or `Advanced::complete_pending_forgets`)
//! finds it and resumes from step 3. Cleanup is idempotent, so resuming is
//! always safe. The one at-least-once edge: a crash between the audit
//! append and the tombstone removal makes the retry append a second
//! `Forget` event for the same memory.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::backend::Record;

pub(crate) const KIND_FORGET_PENDING: &str = "forget_pending";

/// What a pending forget remembers about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingForget {
    pub memory_id: Uuid,
    pub requested_at: DateTime<Utc>,
}

impl PendingForget {
    pub(crate) fn new(memory_id: Uuid) -> Self {
        Self {
            memory_id,
            requested_at: Utc::now(),
        }
    }

    /// The tombstone's own record id, derived from the memory id so a
    /// resume is a single keyed `get`, not a scan. Domain-separated so it
    /// can never equal a memory id.
    pub(crate) fn record_id(memory_id: Uuid) -> Uuid {
        let digest = Sha256::new()
            .chain_update(b"z3rno:forget_pending:")
            .chain_update(memory_id.as_bytes())
            .finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        uuid::Builder::from_random_bytes(bytes).into_uuid()
    }

    pub(crate) fn to_record(&self, tenant_id: &str) -> anyhow::Result<Record> {
        Ok(Record {
            id: Self::record_id(self.memory_id),
            tenant_id: tenant_id.to_string(),
            kind: KIND_FORGET_PENDING.to_string(),
            data: serde_json::to_value(self)?,
            created_at: self.requested_at,
        })
    }

    pub(crate) fn from_record(record: Record) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(record.data)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_id_is_deterministic_and_distinct_from_the_memory_id() {
        let memory_id = Uuid::new_v4();
        assert_eq!(
            PendingForget::record_id(memory_id),
            PendingForget::record_id(memory_id)
        );
        assert_ne!(PendingForget::record_id(memory_id), memory_id);
        assert_ne!(
            PendingForget::record_id(memory_id),
            PendingForget::record_id(Uuid::new_v4())
        );
    }

    #[test]
    fn round_trips_through_a_record() {
        let pending = PendingForget::new(Uuid::new_v4());
        let record = pending.to_record("tenant-a").unwrap();
        assert_eq!(record.kind, KIND_FORGET_PENDING);
        assert_eq!(PendingForget::from_record(record).unwrap(), pending);
    }
}

#[cfg(test)]
mod protocol_tests {
    //! The forget protocol against a real backend set with injected
    //! derived-store failures: embedded always, Postgres when available.

    use std::sync::Arc;

    use serde_json::json;
    use uuid::Uuid;

    use crate::backend::faults::{FaultyGraph, FaultyVector};
    use crate::backend::{
        BackendError, CleanupStep, EmbeddedGraphBackend, EmbeddedVectorBackend, GraphBackend,
        SqliteEngineBackend, VectorBackend,
    };
    use crate::{AuditOperation, BackendTier, MemoryEngine, Tier};

    struct Harness {
        engine: MemoryEngine,
        vector: Arc<FaultyVector>,
        graph: Arc<FaultyGraph>,
    }

    fn embedded() -> Harness {
        let conn = crate::backend::embedded::conn::open_in_memory().unwrap();
        let vector: Arc<dyn VectorBackend> =
            Arc::new(EmbeddedVectorBackend::with_connection(conn.clone()).unwrap());
        let graph: Arc<dyn GraphBackend> =
            Arc::new(EmbeddedGraphBackend::with_connection(conn.clone()).unwrap());
        let vector = Arc::new(FaultyVector::new(vector));
        let graph = Arc::new(FaultyGraph::new(graph));
        let engine = MemoryEngine::new(
            Arc::new(SqliteEngineBackend::with_connection(conn).unwrap()),
            vector.clone(),
            graph.clone(),
            BackendTier::Embedded,
        );
        Harness {
            engine,
            vector,
            graph,
        }
    }

    async fn postgres() -> Harness {
        use crate::backend::postgres::{
            connect, ensure_extensions, PostgresEngineBackend, PostgresGraphBackend,
            PostgresVectorBackend,
        };
        let url = std::env::var("Z3RNO_TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:55433/z3rno_dev".into());
        let pool = connect(&url).await.unwrap();
        ensure_extensions(&pool).await.unwrap();
        let vector: Arc<dyn VectorBackend> =
            Arc::new(PostgresVectorBackend::new(pool.clone()).await.unwrap());
        let graph: Arc<dyn GraphBackend> = Arc::new(PostgresGraphBackend::new(pool.clone()));
        let vector = Arc::new(FaultyVector::new(vector));
        let graph = Arc::new(FaultyGraph::new(graph));
        let engine = MemoryEngine::new(
            Arc::new(PostgresEngineBackend::new(pool).await.unwrap()),
            vector.clone(),
            graph.clone(),
            BackendTier::Postgres,
        );
        Harness {
            engine,
            vector,
            graph,
        }
    }

    fn unit(dims: usize, index: usize) -> Vec<f32> {
        let mut v = vec![0.0; dims];
        v[index] = 1.0;
        v
    }

    /// The shared acceptance scenario, run once per backend tier.
    async fn failing_cleanup_is_reported_and_retry_completes(h: Harness, dims: usize) {
        let tenant = format!("tenant-{}", Uuid::new_v4());
        let e = &h.engine;
        let anchor = e
            .store(
                &tenant,
                Tier::Semantic,
                "anchor".into(),
                Some(unit(dims, 1)),
                json!({}),
                vec![],
            )
            .await
            .unwrap();
        let doomed = e
            .store(
                &tenant,
                Tier::Semantic,
                "doomed".into(),
                Some(unit(dims, 0)),
                json!({}),
                vec![(anchor.id, "about".into())],
            )
            .await
            .unwrap();
        h.graph
            .add_edge(
                &tenant,
                crate::backend::GraphEdge {
                    source: anchor.id,
                    target: doomed.id,
                    relationship: "mentions".into(),
                },
            )
            .await
            .unwrap();

        h.vector.fail_next_deletes(1);
        h.graph.fail_next_removes(1);
        let err = e.forget(&tenant, doomed.id).await.unwrap_err();
        let BackendError::ForgetIncomplete {
            memory_id,
            failures,
        } = err
        else {
            panic!("expected ForgetIncomplete, got {err:?}");
        };
        assert_eq!(memory_id, doomed.id);
        let steps: Vec<_> = failures.iter().map(|f| f.step).collect();
        assert_eq!(steps, vec![CleanupStep::Vector, CleanupStep::Graph]);

        // Hidden from every read path while cleanup is pending.
        let recalled = e.recall(&tenant, unit(dims, 0), 10).await.unwrap();
        assert!(recalled.iter().all(|m| m.id != doomed.id));
        let neighbors = e
            .advanced()
            .neighbors(&tenant, anchor.id, None)
            .await
            .unwrap();
        assert!(
            !neighbors.contains(&doomed.id),
            "neighbors leaked a forgotten id"
        );
        let pending = e.advanced().pending_forgets(&tenant).await.unwrap();
        assert_eq!(
            pending.iter().map(|p| p.memory_id).collect::<Vec<_>>(),
            vec![doomed.id]
        );
        let audit = e.advanced().audit(&tenant).await.unwrap();
        assert!(
            audit
                .iter()
                .all(|ev| ev.operation != AuditOperation::Forget),
            "Forget must not be audited before cleanup completes"
        );

        // A repeat forget resumes the cleanup instead of returning None.
        let proof = e
            .forget(&tenant, doomed.id)
            .await
            .unwrap()
            .expect("proof on retry");
        assert!(h
            .vector
            .search(&tenant, unit(dims, 0), 10)
            .await
            .unwrap()
            .iter()
            .all(|m| m.id != doomed.id));
        assert!(h
            .graph
            .neighbors(&tenant, anchor.id, None)
            .await
            .unwrap()
            .is_empty());
        assert!(e
            .advanced()
            .pending_forgets(&tenant)
            .await
            .unwrap()
            .is_empty());
        let audit = e.advanced().audit(&tenant).await.unwrap();
        let forgets: Vec<_> = audit
            .iter()
            .filter(|ev| ev.operation == AuditOperation::Forget)
            .collect();
        assert_eq!(forgets.len(), 1);
        assert_eq!(forgets[0].id, proof.audit_event_id);

        // Fully done: a third call is a no-op.
        assert!(e.forget(&tenant, doomed.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn embedded_failing_cleanup_is_reported_and_retry_completes() {
        failing_cleanup_is_reported_and_retry_completes(embedded(), 4).await;
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn postgres_failing_cleanup_is_reported_and_retry_completes() {
        failing_cleanup_is_reported_and_retry_completes(postgres().await, 384).await;
    }

    #[tokio::test]
    async fn complete_pending_forgets_finishes_every_pending_memory() {
        let h = embedded();
        let tenant = "tenant-a";
        let mut ids = Vec::new();
        for i in 0..3 {
            let m = h
                .engine
                .store(
                    tenant,
                    Tier::Working,
                    format!("m{i}"),
                    Some(unit(4, i)),
                    json!({}),
                    vec![],
                )
                .await
                .unwrap();
            ids.push(m.id);
        }
        h.vector.fail_next_deletes(3);
        for id in &ids {
            assert!(h.engine.forget(tenant, *id).await.is_err());
        }
        let results = h
            .engine
            .advanced()
            .complete_pending_forgets(tenant)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|(_, r)| r.is_ok()));
        assert!(h
            .engine
            .advanced()
            .pending_forgets(tenant)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn forget_refuses_to_touch_non_memory_records() {
        let h = embedded();
        let m = h
            .engine
            .store(
                "tenant-a",
                Tier::Working,
                "x".into(),
                None,
                json!({}),
                vec![],
            )
            .await
            .unwrap();
        let audit = h.engine.advanced().audit("tenant-a").await.unwrap();
        assert!(h
            .engine
            .forget("tenant-a", audit[0].id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            h.engine.advanced().audit("tenant-a").await.unwrap().len(),
            1
        );
        assert!(h.engine.forget("tenant-a", m.id).await.unwrap().is_some());
    }
}
