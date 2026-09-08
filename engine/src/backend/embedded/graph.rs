//! `GraphBackend` implementation on top of `petgraph`, the zero-infra default
//! for z3rno's relationship storage (episodic links, semantic associations).
//!
//! ## Persistence
//!
//! Option (b) from this module's earlier design note: nodes and edges are
//! folded into the same SQLite file the relational backend already uses
//! (`graph_nodes`/`graph_edges` tables), and `petgraph` is kept as a pure
//! in-memory query/traversal cache rebuilt from those tables on `open`.
//! `Uuid -> NodeIndex` stays process-local and is never serialized (the
//! instability that ruled out plan (a)'s bincode-the-graph approach never
//! comes up, since the cache is always rebuilt from the `Uuid`-keyed rows,
//! never deserialized directly). Every mutating method writes through to
//! SQLite before updating the cache, so the cache never gets ahead of the
//! durable copy on a mid-write failure.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use rusqlite::{params, Connection};
use uuid::Uuid;

use super::conn;
use crate::backend::error::BackendResult;
use crate::backend::graph::{GraphBackend, GraphEdge};

/// One tenant's graph: node weights are the memory node `Uuid`s, edge
/// weights are relationship names. `StableDiGraph` (not `Graph`) is load
/// bearing here — plain `Graph::remove_node` swap-removes, which would
/// silently invalidate every other `Uuid -> NodeIndex` entry in `index`.
#[derive(Default)]
struct TenantGraph {
    graph: StableDiGraph<Uuid, String>,
    index: HashMap<Uuid, NodeIndex>,
}

impl TenantGraph {
    fn get_or_create(&mut self, id: Uuid) -> NodeIndex {
        if let Some(&idx) = self.index.get(&id) {
            return idx;
        }
        let idx = self.graph.add_node(id);
        self.index.insert(id, idx);
        idx
    }
}

/// Embedded, in-process `GraphBackend`. One `petgraph` graph per tenant as a
/// query/traversal cache (`RwLock`, so concurrent `neighbors` reads don't
/// serialize on each other), backed by a SQLite connection every mutating
/// method writes through to first. See module docs for the persistence
/// design.
pub struct EmbeddedGraphBackend {
    conn: Arc<Mutex<Connection>>,
    tenants: RwLock<HashMap<String, TenantGraph>>,
}

impl EmbeddedGraphBackend {
    /// Opens (creating if missing) a SQLite file at `path` on its own
    /// connection, ensures the schema exists, and rebuilds the in-memory
    /// cache from it. For standalone use — `MemoryEngine::embedded` uses
    /// `with_connection` so this shares a connection with the vector and
    /// relational backends instead.
    pub fn open<P: AsRef<Path>>(path: P) -> BackendResult<Self> {
        Self::with_connection(conn::open(path)?)
    }

    /// A backend with no durable file at all — every unit test in this
    /// module's own `tests` submodule uses this, since they only exercise
    /// one process's lifetime and don't need a real file to prove that.
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
                "CREATE TABLE IF NOT EXISTS graph_nodes (
                    tenant_id TEXT NOT NULL,
                    id TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, id)
                );
                CREATE TABLE IF NOT EXISTS graph_edges (
                    tenant_id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    target TEXT NOT NULL,
                    relationship TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_graph_edges_tenant_source
                    ON graph_edges (tenant_id, source);",
            )
            .map_err(anyhow::Error::from)?;
        }
        let tenants = Self::load_all(&conn)?;
        Ok(Self {
            conn,
            tenants: RwLock::new(tenants),
        })
    }

    /// Rebuilds every tenant's `TenantGraph` from the `graph_nodes`/
    /// `graph_edges` tables — the whole point of keeping `petgraph` as a
    /// cache rather than the source of truth.
    fn load_all(conn: &Arc<Mutex<Connection>>) -> BackendResult<HashMap<String, TenantGraph>> {
        let c = conn.lock().unwrap();
        let mut tenants: HashMap<String, TenantGraph> = HashMap::new();

        let mut nodes_stmt = c
            .prepare("SELECT tenant_id, id FROM graph_nodes")
            .map_err(anyhow::Error::from)?;
        let node_rows = nodes_stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(anyhow::Error::from)?;
        for row in node_rows {
            let (tenant_id, id) = row.map_err(anyhow::Error::from)?;
            let id = Uuid::parse_str(&id).map_err(anyhow::Error::from)?;
            tenants.entry(tenant_id).or_default().get_or_create(id);
        }

        let mut edges_stmt = c
            .prepare("SELECT tenant_id, source, target, relationship FROM graph_edges")
            .map_err(anyhow::Error::from)?;
        let edge_rows = edges_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(anyhow::Error::from)?;
        for row in edge_rows {
            let (tenant_id, source, target, relationship) = row.map_err(anyhow::Error::from)?;
            let source = Uuid::parse_str(&source).map_err(anyhow::Error::from)?;
            let target = Uuid::parse_str(&target).map_err(anyhow::Error::from)?;
            let tenant = tenants.entry(tenant_id).or_default();
            let s = tenant.get_or_create(source);
            let t = tenant.get_or_create(target);
            tenant.graph.add_edge(s, t, relationship);
        }

        Ok(tenants)
    }
}

#[async_trait]
impl GraphBackend for EmbeddedGraphBackend {
    async fn add_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<()> {
        {
            let c = self.conn.lock().unwrap();
            c.execute(
                "INSERT OR IGNORE INTO graph_nodes (tenant_id, id) VALUES (?1, ?2)",
                params![tenant_id, id.to_string()],
            )
            .map_err(anyhow::Error::from)?;
        }
        // lock poison is unrecoverable
        let mut tenants = self.tenants.write().unwrap();
        tenants
            .entry(tenant_id.to_string())
            .or_default()
            .get_or_create(id);
        Ok(())
    }

    async fn add_edge(&self, tenant_id: &str, edge: GraphEdge) -> BackendResult<()> {
        {
            let c = self.conn.lock().unwrap();
            // Auto-create endpoints that weren't explicitly added via
            // add_node, matching MockGraphBackend::add_edge — it stores the
            // edge unconditionally without checking the node list first.
            c.execute(
                "INSERT OR IGNORE INTO graph_nodes (tenant_id, id) VALUES (?1, ?2)",
                params![tenant_id, edge.source.to_string()],
            )
            .map_err(anyhow::Error::from)?;
            c.execute(
                "INSERT OR IGNORE INTO graph_nodes (tenant_id, id) VALUES (?1, ?2)",
                params![tenant_id, edge.target.to_string()],
            )
            .map_err(anyhow::Error::from)?;
            c.execute(
                "INSERT INTO graph_edges (tenant_id, source, target, relationship)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    tenant_id,
                    edge.source.to_string(),
                    edge.target.to_string(),
                    edge.relationship,
                ],
            )
            .map_err(anyhow::Error::from)?;
        }
        let mut tenants = self.tenants.write().unwrap();
        let tenant = tenants.entry(tenant_id.to_string()).or_default();
        let source = tenant.get_or_create(edge.source);
        let target = tenant.get_or_create(edge.target);
        tenant.graph.add_edge(source, target, edge.relationship);
        Ok(())
    }

    async fn neighbors(
        &self,
        tenant_id: &str,
        id: Uuid,
        relationship: Option<&str>,
    ) -> BackendResult<Vec<Uuid>> {
        let tenants = self.tenants.read().unwrap();
        let Some(tenant) = tenants.get(tenant_id) else {
            return Ok(Vec::new());
        };
        let Some(&node) = tenant.index.get(&id) else {
            return Ok(Vec::new());
        };
        Ok(tenant
            .graph
            .edges_directed(node, Direction::Outgoing)
            .filter(|e| relationship.is_none_or(|r| r == e.weight()))
            .map(|e| tenant.graph[e.target()])
            .collect())
    }

    async fn remove_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let mut tenants = self.tenants.write().unwrap();
        let Some(tenant) = tenants.get_mut(tenant_id) else {
            return Ok(false);
        };
        let Some(node) = tenant.index.remove(&id) else {
            return Ok(false);
        };
        // StableDiGraph::remove_node also drops every edge incident to the
        // node, and (unlike Graph) leaves every other NodeIndex untouched.
        tenant.graph.remove_node(node);
        drop(tenants);

        let c = self.conn.lock().unwrap();
        c.execute(
            "DELETE FROM graph_nodes WHERE tenant_id = ?1 AND id = ?2",
            params![tenant_id, id.to_string()],
        )
        .map_err(anyhow::Error::from)?;
        c.execute(
            "DELETE FROM graph_edges WHERE tenant_id = ?1 AND (source = ?2 OR target = ?2)",
            params![tenant_id, id.to_string()],
        )
        .map_err(anyhow::Error::from)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_and_traverse_edge() {
        let backend = EmbeddedGraphBackend::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        backend.add_node("tenant-a", a).await.unwrap();
        backend.add_node("tenant-a", b).await.unwrap();
        backend
            .add_edge(
                "tenant-a",
                GraphEdge {
                    source: a,
                    target: b,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();

        let neighbors = backend.neighbors("tenant-a", a, None).await.unwrap();
        assert_eq!(neighbors, vec![b]);

        let filtered = backend
            .neighbors("tenant-a", a, Some("other"))
            .await
            .unwrap();
        assert!(filtered.is_empty());

        let matched = backend
            .neighbors("tenant-a", a, Some("relates_to"))
            .await
            .unwrap();
        assert_eq!(matched, vec![b]);
    }

    #[tokio::test]
    async fn tenants_are_isolated() {
        let backend = EmbeddedGraphBackend::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        backend.add_node("tenant-a", a).await.unwrap();
        backend.add_node("tenant-a", b).await.unwrap();
        backend
            .add_edge(
                "tenant-a",
                GraphEdge {
                    source: a,
                    target: b,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();

        // tenant-b never saw these nodes/edges at all.
        assert!(backend
            .neighbors("tenant-b", a, None)
            .await
            .unwrap()
            .is_empty());
        assert!(!backend.remove_node("tenant-b", a).await.unwrap());
    }

    #[tokio::test]
    async fn remove_node_cleans_up_incident_edges() {
        let backend = EmbeddedGraphBackend::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        backend.add_node("tenant-a", a).await.unwrap();
        backend.add_node("tenant-a", b).await.unwrap();
        backend.add_node("tenant-a", c).await.unwrap();
        backend
            .add_edge(
                "tenant-a",
                GraphEdge {
                    source: a,
                    target: b,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();
        backend
            .add_edge(
                "tenant-a",
                GraphEdge {
                    source: c,
                    target: a,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();

        assert!(backend.remove_node("tenant-a", a).await.unwrap());
        // second removal is a no-op, not an error
        assert!(!backend.remove_node("tenant-a", a).await.unwrap());

        // b and c survive as nodes; the edges touching `a` are gone.
        assert!(backend
            .neighbors("tenant-a", a, None)
            .await
            .unwrap()
            .is_empty());
        assert!(backend
            .neighbors("tenant-a", c, None)
            .await
            .unwrap()
            .is_empty());

        // b and c are still independently usable as graph nodes.
        backend
            .add_edge(
                "tenant-a",
                GraphEdge {
                    source: b,
                    target: c,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            backend.neighbors("tenant-a", b, None).await.unwrap(),
            vec![c]
        );
    }

    #[tokio::test]
    async fn add_edge_auto_creates_missing_nodes() {
        let backend = EmbeddedGraphBackend::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        // Neither node was registered via add_node first.
        backend
            .add_edge(
                "tenant-a",
                GraphEdge {
                    source: a,
                    target: b,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            backend.neighbors("tenant-a", a, None).await.unwrap(),
            vec![b]
        );
    }

    /// The regression this module exists to fix: a graph built, dropped,
    /// then reopened against the same file must not have lost anything.
    #[tokio::test]
    async fn survives_a_reopen_of_the_same_file() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        {
            let backend = EmbeddedGraphBackend::open(db_file.path()).unwrap();
            backend.add_node("tenant-a", a).await.unwrap();
            backend
                .add_edge(
                    "tenant-a",
                    GraphEdge {
                        source: a,
                        target: b,
                        relationship: "relates_to".into(),
                    },
                )
                .await
                .unwrap();
            backend.add_node("tenant-a", c).await.unwrap();
            backend.remove_node("tenant-a", c).await.unwrap();
        }

        // A fresh backend against the same file, as if the process
        // restarted — this used to come back empty.
        let reopened = EmbeddedGraphBackend::open(db_file.path()).unwrap();
        assert_eq!(
            reopened.neighbors("tenant-a", a, None).await.unwrap(),
            vec![b]
        );
        // The removed node's row didn't survive either.
        assert!(!reopened.remove_node("tenant-a", c).await.unwrap());
    }
}
