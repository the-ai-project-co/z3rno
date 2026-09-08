//! Embedded `VectorBackend`: an in-process approximate-nearest-neighbor
//! index, no external service.
//!
//! ## Crate choice: `hnsw_rs` over `instant-distance`
//!
//! Both candidates named in the plan doc were spiked against their docs.rs
//! pages (and, for `hnsw_rs`, against a real standalone compile — see
//! below). `instant-distance`'s public API (`Builder` -> `Hnsw`) is a
//! one-shot build from a fixed point set with no documented post-build
//! insert or delete — exactly the "build-once/immutable" risk the plan doc
//! called out. `hnsw_rs` (0.3.4) does expose an `insert()` on an already-
//! built index, is more thoroughly documented, and is the more actively
//! maintained crate, so it was picked. `hnsw_rs::prelude::{Hnsw, DistCosine}`
//! was compiled and run standalone against a tiny 3-vector fixture outside
//! this workspace to confirm the exact API (`Hnsw::new` params, `insert`,
//! `search`, `Neighbour::{d_id, distance}`) before writing this file.
//!
//! ## Why the index is rebuilt on every search, not kept resident
//!
//! `hnsw_rs` supports incremental *insert*, but has no delete/remove — HNSW
//! graphs are hard to delete from in general (removing a node risks
//! disconnecting the graph), and this crate doesn't expose it. Its
//! `Hnsw<'b, T, D>` also *borrows* the vector slices it indexes (lifetime
//! `'b`), so keeping a live index next to its own owned backing storage in
//! one struct is a self-referential-struct problem in safe Rust.
//!
//! Given neither incremental delete nor a free self-referential design is
//! available, this backend keeps the plain vectors as the single source of
//! truth per tenant (`HashMap<Uuid, Vec<f32>>`) and builds a fresh `Hnsw`
//! from scratch inside every `search()` call, scoped entirely to that call.
//! `upsert`/`delete` then become trivial `HashMap` mutations with no index
//! to keep in sync, and the index can never go stale since it never
//! persists across calls.
//!
//! ponytail: rebuilding per search is O(n log n) per query instead of
//! O(log n) against a persisted index. That's fine at the embedded
//! backend's expected scale (a tenant's local working set, not a bulk
//! corpus — pgvector is the production backend for that). Upgrade path if
//! profiling ever shows this dominates: cache a built index per tenant
//! behind a generation counter, invalidated on upsert/delete, using either
//! `unsafe` or an owning-borrow wrapper crate (e.g. `ouroboros`) to hold the
//! borrowed index next to its data.
//!
//! ## Score conversion
//!
//! `DistCosine` returns a *distance* (lower = more similar); the trait's
//! `VectorMatch::score` must be a *similarity* (higher = more similar).
//! Converted with `1.0 / (1.0 + distance)`, which is monotonically
//! decreasing in distance for any non-negative distance, so nearest-
//! neighbor ranking is preserved regardless of `DistCosine`'s exact numeric
//! range.
//!
//! ## Persistence
//!
//! The `HashMap<Uuid, Vec<f32>>` per tenant above is a cache, not the
//! source of truth: every embedding is also written to a `vectors` table in
//! the same SQLite file the relational and graph backends use, as a raw
//! little-endian `f32` byte blob (no serde/bincode framing needed for a
//! flat float slice). `open`/`open_in_memory` load every row back into the
//! cache before the backend does anything else, so a process restart no
//! longer loses the index — only the fresh-per-search HNSW build stays
//! purely in-memory, unaffected by any of this.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use hnsw_rs::prelude::*;
use rusqlite::{params, Connection};
use uuid::Uuid;

use super::conn;
use crate::backend::error::BackendResult;
use crate::backend::vector::{VectorBackend, VectorMatch};

fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().expect("chunks_exact(4) yields 4 bytes")))
        .collect()
}

const MAX_NB_CONNECTION: usize = 16;
const EF_CONSTRUCTION: usize = 200;
const MAX_LAYER: usize = 16;

/// Per-tenant plain-vector store; the ANN index is built fresh per search
/// (see module docs) rather than kept resident.
///
/// `Mutex`, not `RwLock`: every method here briefly locks, does bounded
/// work, and returns — read/write contention isn't expected to be high
/// enough at embedded scale for `RwLock`'s extra complexity to pay for
/// itself, and this matches `mock.rs`'s pattern.
pub struct EmbeddedVectorBackend {
    conn: Arc<Mutex<Connection>>,
    tenants: Mutex<HashMap<String, HashMap<Uuid, Vec<f32>>>>,
}

impl EmbeddedVectorBackend {
    /// Opens (creating if missing) a SQLite file at `path` on its own
    /// connection, ensures the schema exists, and loads every stored
    /// embedding into the cache. For standalone use — `MemoryEngine::
    /// embedded` uses `with_connection` so this shares a connection with
    /// the relational and graph backends instead.
    pub fn open<P: AsRef<Path>>(path: P) -> BackendResult<Self> {
        Self::with_connection(conn::open(path)?)
    }

    /// A backend with no durable file at all — this module's own unit
    /// tests use this, since they only exercise one process's lifetime.
    pub fn open_in_memory() -> BackendResult<Self> {
        Self::with_connection(conn::open_in_memory()?)
    }

    /// Same as `open`, but against an existing shared connection (already
    /// pointed at a real file or `:memory:`) rather than opening its own.
    pub(crate) fn with_connection(conn: Arc<Mutex<Connection>>) -> BackendResult<Self> {
        {
            // lock poison is unrecoverable
            let c = conn.lock().unwrap();
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS vectors (
                    tenant_id TEXT NOT NULL,
                    id TEXT NOT NULL,
                    embedding BLOB NOT NULL,
                    PRIMARY KEY (tenant_id, id)
                );",
            )
            .map_err(anyhow::Error::from)?;
        }
        let tenants = Self::load_all(&conn)?;
        Ok(Self {
            conn,
            tenants: Mutex::new(tenants),
        })
    }

    fn load_all(
        conn: &Arc<Mutex<Connection>>,
    ) -> BackendResult<HashMap<String, HashMap<Uuid, Vec<f32>>>> {
        let c = conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT tenant_id, id, embedding FROM vectors")
            .map_err(anyhow::Error::from)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(anyhow::Error::from)?;

        let mut tenants: HashMap<String, HashMap<Uuid, Vec<f32>>> = HashMap::new();
        for row in rows {
            let (tenant_id, id, embedding) = row.map_err(anyhow::Error::from)?;
            let id = Uuid::parse_str(&id).map_err(anyhow::Error::from)?;
            tenants
                .entry(tenant_id)
                .or_default()
                .insert(id, decode_embedding(&embedding));
        }
        Ok(tenants)
    }
}

fn search_tenant(vectors: &HashMap<Uuid, Vec<f32>>, query: &[f32], k: usize) -> Vec<VectorMatch> {
    if vectors.is_empty() || k == 0 {
        return Vec::new();
    }

    let ids: Vec<Uuid> = vectors.keys().copied().collect();
    let index: Hnsw<f32, DistCosine> = Hnsw::new(
        MAX_NB_CONNECTION,
        ids.len(),
        MAX_LAYER,
        EF_CONSTRUCTION,
        DistCosine,
    );
    for (i, id) in ids.iter().enumerate() {
        index.insert((vectors[id].as_slice(), i));
    }

    let ef_search = EF_CONSTRUCTION.max(k);
    let mut matches: Vec<VectorMatch> = index
        .search(query, k, ef_search)
        .into_iter()
        .map(|neighbour| VectorMatch {
            id: ids[neighbour.d_id],
            score: 1.0 / (1.0 + neighbour.distance),
        })
        .collect();
    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    matches.truncate(k);
    matches
}

#[async_trait]
impl VectorBackend for EmbeddedVectorBackend {
    async fn upsert(&self, tenant_id: &str, id: Uuid, embedding: Vec<f32>) -> BackendResult<()> {
        {
            let c = self.conn.lock().unwrap();
            c.execute(
                "INSERT INTO vectors (tenant_id, id, embedding) VALUES (?1, ?2, ?3)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET embedding = excluded.embedding",
                params![tenant_id, id.to_string(), encode_embedding(&embedding)],
            )
            .map_err(anyhow::Error::from)?;
        }
        // lock poison is unrecoverable
        self.tenants
            .lock()
            .unwrap()
            .entry(tenant_id.to_string())
            .or_default()
            .insert(id, embedding);
        Ok(())
    }

    async fn search(
        &self,
        tenant_id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> BackendResult<Vec<VectorMatch>> {
        let tenants = self.tenants.lock().unwrap();
        let Some(vectors) = tenants.get(tenant_id) else {
            return Ok(Vec::new());
        };
        Ok(search_tenant(vectors, &query, k))
    }

    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let removed = {
            let mut tenants = self.tenants.lock().unwrap();
            tenants
                .get_mut(tenant_id)
                .is_some_and(|vectors| vectors.remove(&id).is_some())
        };
        if removed {
            let c = self.conn.lock().unwrap();
            c.execute(
                "DELETE FROM vectors WHERE tenant_id = ?1 AND id = ?2",
                params![tenant_id, id.to_string()],
            )
            .map_err(anyhow::Error::from)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn search_returns_nearest_neighbor() {
        let backend = EmbeddedVectorBackend::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        backend
            .upsert("tenant-a", a, vec![1.0, 0.0, 0.0])
            .await
            .unwrap();
        backend
            .upsert("tenant-a", b, vec![0.0, 1.0, 0.0])
            .await
            .unwrap();
        backend
            .upsert("tenant-a", c, vec![0.0, 0.0, 1.0])
            .await
            .unwrap();

        let results = backend
            .search("tenant-a", vec![0.9, 0.1, 0.0], 1)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, a);
    }

    #[tokio::test]
    async fn search_is_tenant_isolated() {
        let backend = EmbeddedVectorBackend::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        backend
            .upsert("tenant-a", a, vec![1.0, 0.0, 0.0])
            .await
            .unwrap();
        backend
            .upsert("tenant-b", b, vec![1.0, 0.0, 0.0])
            .await
            .unwrap();

        let results = backend
            .search("tenant-a", vec![1.0, 0.0, 0.0], 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, a);
    }

    #[tokio::test]
    async fn delete_removes_vector_from_search() {
        let backend = EmbeddedVectorBackend::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        backend
            .upsert("tenant-a", a, vec![1.0, 0.0, 0.0])
            .await
            .unwrap();
        backend
            .upsert("tenant-a", b, vec![0.0, 1.0, 0.0])
            .await
            .unwrap();

        assert!(backend.delete("tenant-a", a).await.unwrap());
        assert!(!backend.delete("tenant-a", a).await.unwrap());

        let results = backend
            .search("tenant-a", vec![1.0, 0.0, 0.0], 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, b);
    }

    /// The regression this module exists to fix: store an embedding, drop
    /// the backend, reconstruct against the same file, and confirm search
    /// still finds it — this used to come back empty (issue #21).
    #[tokio::test]
    async fn survives_a_reopen_of_the_same_file() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let a = Uuid::new_v4();
        {
            let backend = EmbeddedVectorBackend::open(db_file.path()).unwrap();
            backend
                .upsert("tenant-a", a, vec![1.0, 0.0, 0.0])
                .await
                .unwrap();
        }

        let reopened = EmbeddedVectorBackend::open(db_file.path()).unwrap();
        let results = reopened
            .search("tenant-a", vec![0.9, 0.1, 0.0], 1)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, a);
    }
}
