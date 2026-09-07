//! The two-tier verb surface (0003.6): `store`/`recall`/`forget` as the
//! documented default (decision-doc 0003's pattern, applied at the engine
//! layer so the bindings don't have to reinvent the namespacing), with
//! `audit` — and any future advanced operation — reachable only through
//! `MemoryEngine::advanced()`, not as a fourth top-level method.

use std::path::Path;
use std::sync::Arc;

use uuid::Uuid;

use crate::audit::{AuditEvent, AuditOperation, ForgetProof};
use crate::backend::{
    BackendResult, EmbeddedGraphBackend, EmbeddedVectorBackend, EngineBackend, GraphBackend,
    Record, SqliteEngineBackend, VectorBackend,
};
use crate::model::{Memory, Tier};

const KIND_MEMORY: &str = "memory";
const KIND_AUDIT_EVENT: &str = "audit_event";

/// The core memory engine: `store`/`recall`/`forget` against whichever
/// backends it's built with — embedded (see `MemoryEngine::embedded`) or,
/// from slice 0004 on, the production Postgres+pgvector+AGE backend.
/// Callers never depend on a concrete backend, only this struct and the
/// trait interfaces it's built from.
pub struct MemoryEngine {
    engine_backend: Arc<dyn EngineBackend>,
    vector_backend: Arc<dyn VectorBackend>,
    graph_backend: Arc<dyn GraphBackend>,
}

impl MemoryEngine {
    pub fn new(
        engine_backend: Arc<dyn EngineBackend>,
        vector_backend: Arc<dyn VectorBackend>,
        graph_backend: Arc<dyn GraphBackend>,
    ) -> Self {
        Self {
            engine_backend,
            vector_backend,
            graph_backend,
        }
    }

    /// The zero-infra default: a `MemoryEngine` backed entirely by embedded
    /// storage (SQLite + an in-process vector index + an in-process graph),
    /// no external services. This is what makes `pip install z3rno` /
    /// `npm install @z3rno/sdk` work out of the box.
    pub fn embedded<P: AsRef<Path>>(sqlite_path: P) -> BackendResult<Self> {
        Ok(Self::new(
            Arc::new(SqliteEngineBackend::open(sqlite_path)?),
            Arc::new(EmbeddedVectorBackend::default()),
            Arc::new(EmbeddedGraphBackend::default()),
        ))
    }

    /// Stores a memory. `embedding` is optional (a memory with no vector
    /// simply isn't recallable by similarity search, only by direct id or
    /// graph traversal); `links` are optional graph edges from this memory
    /// to other existing memory ids (episodic links, semantic associations
    /// — the caller decides the relationship name).
    pub async fn store(
        &self,
        tenant_id: &str,
        tier: Tier,
        content: String,
        embedding: Option<Vec<f32>>,
        metadata: serde_json::Value,
        links: Vec<(Uuid, String)>,
    ) -> BackendResult<Memory> {
        let memory = Memory {
            id: Uuid::new_v4(),
            tenant_id: tenant_id.to_string(),
            tier,
            content,
            metadata,
            created_at: chrono::Utc::now(),
        };

        self.engine_backend
            .put(Record {
                id: memory.id,
                tenant_id: memory.tenant_id.clone(),
                kind: KIND_MEMORY.to_string(),
                data: serde_json::to_value(&memory).map_err(anyhow::Error::from)?,
                created_at: memory.created_at,
            })
            .await?;

        if let Some(embedding) = embedding {
            self.vector_backend
                .upsert(tenant_id, memory.id, embedding)
                .await?;
        }

        self.graph_backend.add_node(tenant_id, memory.id).await?;
        for (target, relationship) in links {
            self.graph_backend
                .add_edge(
                    tenant_id,
                    crate::backend::GraphEdge {
                        source: memory.id,
                        target,
                        relationship,
                    },
                )
                .await?;
        }

        self.append_audit_event(tenant_id, AuditOperation::Store, memory.id)
            .await?;

        Ok(memory)
    }

    /// Recalls up to `k` memories most similar to `query`, ranked by the
    /// vector backend's similarity score.
    pub async fn recall(
        &self,
        tenant_id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> BackendResult<Vec<Memory>> {
        let matches = self.vector_backend.search(tenant_id, query, k).await?;
        let mut memories = Vec::with_capacity(matches.len());
        for m in matches {
            if let Some(record) = self.engine_backend.get(tenant_id, m.id).await? {
                memories.push(Self::record_to_memory(record)?);
            }
        }
        Ok(memories)
    }

    /// Removes a memory from every backend it may live in (relational,
    /// vector, graph) and appends a `Forget` audit event as proof the
    /// erasure happened. Returns `None` if there was nothing to forget
    /// (already forgotten, or never existed) — no audit event is appended
    /// for a no-op.
    pub async fn forget(&self, tenant_id: &str, id: Uuid) -> BackendResult<Option<ForgetProof>> {
        let deleted = self.engine_backend.delete(tenant_id, id).await?;
        if !deleted {
            return Ok(None);
        }
        // Best-effort cleanup in the other backends — the relational store
        // is the source of truth for "does this memory exist"; a vector or
        // graph entry outliving its memory row is inert (never returned by
        // `recall`, which joins back through the relational store) rather
        // than a correctness problem.
        let _ = self.vector_backend.delete(tenant_id, id).await;
        let _ = self.graph_backend.remove_node(tenant_id, id).await;

        let event = self
            .append_audit_event(tenant_id, AuditOperation::Forget, id)
            .await?;
        Ok(Some(ForgetProof {
            audit_event_id: event.id,
            hash: event.hash,
        }))
    }

    /// Advanced operations, kept out of the top-level three verbs per
    /// decision-doc 0003's two-tier pattern — currently just `audit`.
    pub fn advanced(&self) -> Advanced<'_> {
        Advanced { engine: self }
    }

    async fn append_audit_event(
        &self,
        tenant_id: &str,
        operation: AuditOperation,
        memory_id: Uuid,
    ) -> BackendResult<AuditEvent> {
        let prev_hash = self.latest_audit_hash(tenant_id).await?;
        let event = AuditEvent::next(tenant_id, operation, memory_id, prev_hash);
        self.engine_backend
            .put(Record {
                id: event.id,
                tenant_id: tenant_id.to_string(),
                kind: KIND_AUDIT_EVENT.to_string(),
                data: serde_json::to_value(&event).map_err(anyhow::Error::from)?,
                created_at: event.at,
            })
            .await?;
        Ok(event)
    }

    // ponytail: O(n) scan of the tenant's whole audit log to find the chain
    // tip. Fine at embedded scale (a local, per-process log); the
    // production backend (slice 0004) should track the tip directly
    // instead of re-deriving it by scanning every event on every write.
    async fn latest_audit_hash(&self, tenant_id: &str) -> BackendResult<Option<String>> {
        let events = self.advanced().audit(tenant_id).await?;
        Ok(events.last().map(|e| e.hash.clone()))
    }

    fn record_to_memory(record: Record) -> BackendResult<Memory> {
        let memory: Memory = serde_json::from_value(record.data).map_err(anyhow::Error::from)?;
        Ok(memory)
    }
}

/// Advanced operations namespace — `engine.advanced().audit(...)`, never a
/// top-level `MemoryEngine` method.
pub struct Advanced<'a> {
    engine: &'a MemoryEngine,
}

impl Advanced<'_> {
    /// The tenant's full audit chain, oldest first.
    pub async fn audit(&self, tenant_id: &str) -> BackendResult<Vec<AuditEvent>> {
        let records = self
            .engine
            .engine_backend
            .list(tenant_id, KIND_AUDIT_EVENT)
            .await?;
        let mut events = records
            .into_iter()
            .map(|r| serde_json::from_value::<AuditEvent>(r.data).map_err(anyhow::Error::from))
            .collect::<Result<Vec<_>, _>>()?;
        events.sort_by_key(|e| e.at);
        Ok(events)
    }
}
