//! The two-tier verb surface (0003.6): `store`/`recall`/`forget` as the
//! documented default (decision-doc 0003's pattern, applied at the engine
//! layer so the bindings don't have to reinvent the namespacing), with
//! `audit` — and any future advanced operation — reachable only through
//! `MemoryEngine::advanced()`, not as a fourth top-level method.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use uuid::Uuid;

use crate::audit::{AuditEvent, AuditOperation, ForgetProof};
use crate::backend::{
    BackendError, BackendResult, CleanupFailure, CleanupStep, EmbeddedGraphBackend,
    EmbeddedVectorBackend, EngineBackend, GraphBackend, Record, SqliteEngineBackend, VectorBackend,
};
use crate::forget::{PendingForget, KIND_FORGET_PENDING};
use crate::model::{Memory, Tier};

pub(crate) const KIND_MEMORY: &str = "memory";
const KIND_AUDIT_EVENT: &str = "audit_event";

/// Which tenant-isolation guarantee a `MemoryEngine`'s backends provide —
/// see `COMPATIBILITY.md` for the full matrix and reasoning. Set by
/// whichever constructor built the engine (`embedded`/`postgres`), never
/// inferred at runtime — the trait objects `MemoryEngine` holds don't
/// self-identify which concrete backend implements them, so there's no way
/// to derive this after the fact, only to declare it at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendTier {
    /// SQLite + an in-process vector index + an in-process graph. Every
    /// embedded backend does scope its own storage by `tenant_id`, but
    /// that's application code enforcing it, not the storage engine — a
    /// bug in that code is nothing but a bug, with no RLS-style second
    /// layer catching it. Fine for local, single-operator use; not a
    /// guarantee to build a shared multi-tenant deployment on.
    Embedded,
    /// Postgres + pgvector + Apache AGE. Tenant isolation is enforced by
    /// the storage engine itself — Postgres RLS on the relational/vector
    /// tables (`FORCE ROW LEVEL SECURITY`, so it applies even to the
    /// provisioning role), a separate AGE graph per tenant for graph data
    /// (decision-doc 0008's RLS-doesn't-cover-AGE gap, resolved structurally
    /// rather than by filter discipline — see `backend::postgres::graph`'s
    /// module doc). Compliance-grade.
    Postgres,
}

/// The core memory engine: `store`/`recall`/`forget` against whichever
/// backends it's built with — embedded (see `MemoryEngine::embedded`) or
/// the production Postgres+pgvector+AGE backend (`MemoryEngine::postgres`).
/// Callers never depend on a concrete backend, only this struct and the
/// trait interfaces it's built from.
pub struct MemoryEngine {
    // pub(crate) so typed-record modules (lessons) can compose the same
    // backends; never exposed outside the crate.
    pub(crate) engine_backend: Arc<dyn EngineBackend>,
    pub(crate) vector_backend: Arc<dyn VectorBackend>,
    pub(crate) graph_backend: Arc<dyn GraphBackend>,
    tier: BackendTier,
}

impl MemoryEngine {
    /// Deliberately not `pub`: 0004.4's "refuses to start on an
    /// unsupported backend combination" requirement is met here by making
    /// an unsupported combo unconstructable through the public API at all,
    /// rather than a runtime check — `embedded`/`postgres` below are the
    /// only ways to build a `MemoryEngine` from outside this crate, and
    /// each always assembles a complete, matched, documented tier. There's
    /// no independent per-concern backend selection yet (mixing, say, the
    /// embedded vector store with the Postgres relational store) for a
    /// runtime check to guard against — if that's ever built, *that's*
    /// the point to add the runtime check this constructor doesn't have.
    /// `pub(crate)` only so in-crate tests can wrap a real backend in a
    /// fault-injecting decorator; still unreachable from outside the crate.
    pub(crate) fn new(
        engine_backend: Arc<dyn EngineBackend>,
        vector_backend: Arc<dyn VectorBackend>,
        graph_backend: Arc<dyn GraphBackend>,
        tier: BackendTier,
    ) -> Self {
        Self {
            engine_backend,
            vector_backend,
            graph_backend,
            tier,
        }
    }

    /// The zero-infra default: a `MemoryEngine` backed entirely by embedded
    /// storage (SQLite + an in-process vector index + an in-process graph),
    /// no external services. This is what makes `pip install z3rno` /
    /// `npm install @z3rno/sdk` work out of the box.
    ///
    /// All three backends share one connection to `sqlite_path` (see
    /// `backend::embedded::conn`) rather than each opening their own — one
    /// file, one connection, matching the embedded module's own stated
    /// design. Every one of them now persists across a restart: the
    /// relational store always did, and the vector/graph backends fold
    /// their state into the same file as of this constructor (previously
    /// tracked as issues #21 and #8 — both in-memory-only).
    pub fn embedded<P: AsRef<Path>>(sqlite_path: P) -> BackendResult<Self> {
        let conn = crate::backend::embedded::open_shared_connection(sqlite_path)?;
        Ok(Self::new(
            Arc::new(SqliteEngineBackend::with_connection(conn.clone())?),
            Arc::new(EmbeddedVectorBackend::with_connection(conn.clone())?),
            Arc::new(EmbeddedGraphBackend::with_connection(conn)?),
            BackendTier::Embedded,
        ))
    }

    /// The production backend: Postgres + pgvector + Apache AGE (decision-
    /// doc 0008). Auto-provisions the required extensions against
    /// `database_url` (decision-doc 0002 — no bespoke image required) and
    /// **refuses to start if provisioning fails** — this returns an `Err`,
    /// never a partially-working engine, matching 0004.4's "hard error, not
    /// a silent degraded mode" requirement. See
    /// `backend::postgres::BOOTSTRAP_SQL` for the standalone script a DBA
    /// can run when the connecting role can't provision extensions itself.
    pub async fn postgres(database_url: &str) -> BackendResult<Self> {
        let pool = crate::backend::postgres::connect(database_url).await?;
        crate::backend::postgres::ensure_extensions(&pool).await?;
        let engine_backend = crate::backend::postgres::PostgresEngineBackend::new(pool.clone())
            .await
            .map(Arc::new)?;
        let vector_backend = crate::backend::postgres::PostgresVectorBackend::new(pool.clone())
            .await
            .map(Arc::new)?;
        let graph_backend = Arc::new(crate::backend::postgres::PostgresGraphBackend::new(pool));
        Ok(Self::new(
            engine_backend,
            vector_backend,
            graph_backend,
            BackendTier::Postgres,
        ))
    }

    /// Which tenant-isolation guarantee this engine's backends provide —
    /// see [`BackendTier`].
    pub fn tier(&self) -> BackendTier {
        self.tier
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
        self.store_memory(memory, embedding, links).await
    }

    /// Persists an already-built `Memory` (relational row, optional vector,
    /// graph node and links) and audits the store. Shared by `store` and
    /// typed records (lessons) that need to know their id up front.
    pub(crate) async fn store_memory(
        &self,
        memory: Memory,
        embedding: Option<Vec<f32>>,
        links: Vec<(Uuid, String)>,
    ) -> BackendResult<Memory> {
        let tenant_id = memory.tenant_id.as_str();
        self.put_memory(&memory).await?;

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

    /// Writes (or overwrites in place) a memory's relational row.
    pub(crate) async fn put_memory(&self, memory: &Memory) -> BackendResult<()> {
        self.engine_backend
            .put(Record {
                id: memory.id,
                tenant_id: memory.tenant_id.clone(),
                kind: KIND_MEMORY.to_string(),
                data: serde_json::to_value(memory).map_err(anyhow::Error::from)?,
                created_at: memory.created_at,
            })
            .await
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
        if matches.is_empty() {
            return Ok(Vec::new());
        }
        let ids = matches.iter().map(|item| item.id).collect::<Vec<_>>();
        let records = self.engine_backend.get_many(tenant_id, &ids).await?;
        let mut by_id = records
            .into_iter()
            .map(|record| (record.id, record))
            .collect::<HashMap<_, _>>();
        let mut memories = Vec::with_capacity(matches.len());
        for m in matches {
            if let Some(record) = by_id.remove(&m.id) {
                memories.push(Self::record_to_memory(record)?);
            }
        }
        Ok(memories)
    }

    /// Removes a memory from every backend it may live in (relational,
    /// vector, graph). See `crate::forget` for the resumable protocol.
    ///
    /// - `Ok(Some(proof))`: every store is clean and a `Forget` audit event
    ///   was appended. The proof is an audit reference, not an erasure
    ///   certificate: it records that the engine ran the deletion.
    /// - `Ok(None)`: nothing to forget (never existed, already fully
    ///   forgotten, or the id isn't a memory) — no audit event.
    /// - `Err(BackendError::ForgetIncomplete { .. })`: the memory is hidden
    ///   from every read path, but a derived store still holds an entry.
    ///   No audit event yet; call `forget` again to retry.
    pub async fn forget(&self, tenant_id: &str, id: Uuid) -> BackendResult<Option<ForgetProof>> {
        let pending = match self.engine_backend.get(tenant_id, id).await? {
            Some(record) if record.kind == KIND_MEMORY => {
                let pending = PendingForget::new(id);
                // Intent first: once this row exists, every later failure
                // is resumable.
                self.engine_backend
                    .put(pending.to_record(tenant_id)?)
                    .await?;
                self.engine_backend.delete(tenant_id, id).await?;
                pending
            }
            Some(_) => return Ok(None),
            None => match self.pending_forget(tenant_id, id).await? {
                Some(pending) => pending,
                None => return Ok(None),
            },
        };
        self.finish_forget(tenant_id, &pending).await.map(Some)
    }

    async fn pending_forget(
        &self,
        tenant_id: &str,
        memory_id: Uuid,
    ) -> BackendResult<Option<PendingForget>> {
        let record = self
            .engine_backend
            .get(tenant_id, PendingForget::record_id(memory_id))
            .await?;
        match record {
            Some(record) if record.kind == KIND_FORGET_PENDING => {
                Ok(Some(PendingForget::from_record(record)?))
            }
            _ => Ok(None),
        }
    }

    /// Steps 3 and 4 of the forget protocol. Idempotent: both derived
    /// deletes treat "already absent" as success.
    async fn finish_forget(
        &self,
        tenant_id: &str,
        pending: &PendingForget,
    ) -> BackendResult<ForgetProof> {
        let id = pending.memory_id;
        let mut failures = Vec::new();
        if let Err(err) = self.vector_backend.delete(tenant_id, id).await {
            failures.push(CleanupFailure {
                step: CleanupStep::Vector,
                message: err.to_string(),
            });
        }
        if let Err(err) = self.graph_backend.remove_node(tenant_id, id).await {
            failures.push(CleanupFailure {
                step: CleanupStep::Graph,
                message: err.to_string(),
            });
        }
        if !failures.is_empty() {
            return Err(BackendError::ForgetIncomplete {
                memory_id: id,
                failures,
            });
        }

        let event = self
            .append_audit_event(tenant_id, AuditOperation::Forget, id)
            .await?;
        self.engine_backend
            .delete(tenant_id, PendingForget::record_id(id))
            .await?;
        Ok(ForgetProof {
            audit_event_id: event.id,
            hash: event.hash,
        })
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
        self.engine_backend
            .append_audit_event(tenant_id, operation, memory_id)
            .await
    }

    /// Keeps only the ids that still have a memory row, in input order.
    pub(crate) async fn live_memory_ids(
        &self,
        tenant_id: &str,
        ids: Vec<Uuid>,
    ) -> BackendResult<Vec<Uuid>> {
        if ids.is_empty() {
            return Ok(ids);
        }
        let live = self
            .engine_backend
            .get_many(tenant_id, &ids)
            .await?
            .into_iter()
            .filter(|r| r.kind == KIND_MEMORY)
            .map(|r| r.id)
            .collect::<std::collections::HashSet<_>>();
        Ok(ids.into_iter().filter(|id| live.contains(id)).collect())
    }

    pub(crate) fn record_to_memory(record: Record) -> BackendResult<Memory> {
        let memory: Memory = serde_json::from_value(record.data).map_err(anyhow::Error::from)?;
        Ok(memory)
    }
}

/// Advanced operations namespace — `engine.advanced().audit(...)`, never a
/// top-level `MemoryEngine` method.
pub struct Advanced<'a> {
    pub(crate) engine: &'a MemoryEngine,
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

    /// Every memory stored for `tenant_id` — 0010.2's graph visualizer
    /// needs a starting node set, and `recall` can't provide one (it's
    /// vector-similarity-only, requires a query embedding). Wraps the
    /// engine backend's existing `list`, same as `audit` above.
    pub async fn list_memories(&self, tenant_id: &str) -> BackendResult<Vec<Memory>> {
        let records = self
            .engine
            .engine_backend
            .list(tenant_id, KIND_MEMORY)
            .await?;
        records
            .into_iter()
            .map(MemoryEngine::record_to_memory)
            .collect()
    }

    /// One-hop neighbor ids of `id`, optionally filtered by relationship
    /// name. Ids are joined back through the relational store, so a graph
    /// entry whose memory was forgotten (cleanup still pending) is never
    /// returned.
    pub async fn neighbors(
        &self,
        tenant_id: &str,
        id: Uuid,
        relationship: Option<&str>,
    ) -> BackendResult<Vec<Uuid>> {
        let ids = self
            .engine
            .graph_backend
            .neighbors(tenant_id, id, relationship)
            .await?;
        self.engine.live_memory_ids(tenant_id, ids).await
    }

    /// Memory ids for which a `forget` was started but its derived-store
    /// cleanup hasn't finished.
    pub async fn pending_forgets(&self, tenant_id: &str) -> BackendResult<Vec<PendingForget>> {
        let records = self
            .engine
            .engine_backend
            .list(tenant_id, KIND_FORGET_PENDING)
            .await?;
        let mut pending = records
            .into_iter()
            .map(|r| PendingForget::from_record(r).map_err(BackendError::from))
            .collect::<Result<Vec<_>, _>>()?;
        pending.sort_by_key(|p| p.requested_at);
        Ok(pending)
    }

    /// Retries every pending forget for `tenant_id` (e.g. at startup).
    /// Returns one result per pending memory, in request order.
    pub async fn complete_pending_forgets(
        &self,
        tenant_id: &str,
    ) -> BackendResult<Vec<(Uuid, BackendResult<ForgetProof>)>> {
        let mut results = Vec::new();
        for pending in self.pending_forgets(tenant_id).await? {
            let outcome = self.engine.finish_forget(tenant_id, &pending).await;
            results.push((pending.memory_id, outcome));
        }
        Ok(results)
    }
}
