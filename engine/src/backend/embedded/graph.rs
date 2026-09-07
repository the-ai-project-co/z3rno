//! `GraphBackend` implementation on top of `petgraph`, the zero-infra default
//! for z3rno's relationship storage (episodic links, semantic associations).
//!
//! ## Persistence: known gap, deliberately deferred
//!
//! This is in-memory only — a process restart loses every node and edge.
//! That's a real gap against z3rno's "persistent memory" premise, and it is
//! *not* silently accepted:
//!
//! - The trait this implements (`GraphBackend`) has no `flush`/`load`/`path`
//!   method, and the acceptance criterion for this sub-slice (0003.4) is only
//!   "create two memory nodes, link them, traverse the relationship" — it
//!   does not require surviving a restart.
//! - `petgraph`'s own node indices aren't stable across a serialize/reload
//!   cycle unless the `Uuid -> NodeIndex` map is serialized alongside the
//!   graph, which is easy to get subtly wrong (stale indices after a
//!   `remove_node`, partial writes on crash mid-flush).
//! - The relational half of the embedded backend (0003.2, `rusqlite`) is
//!   already the durable store for memory content. Bolting file-based
//!   serialization onto this graph now, with no caller-facing flush point in
//!   the trait and no agreed file layout, would be built to a guess.
//!
//! **Follow-up needed before ship:** either (a) extend `GraphBackend` with an
//! explicit persistence hook and back this with `serde`/`bincode` snapshots,
//! or (b) fold edges into the SQLite store from 0003.2 as an adjacency table
//! and drop `petgraph` to a pure query/traversal layer over that. Flagging
//! this here rather than deciding it unilaterally, since it changes the
//! trait contract other sub-slices depend on.

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use uuid::Uuid;

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

/// Embedded, in-process `GraphBackend`. One `petgraph` graph per tenant,
/// guarded by an `RwLock` so concurrent `neighbors` reads don't serialize on
/// each other (writes — `add_node`/`add_edge`/`remove_node` — still take the
/// write lock). See module docs for the persistence gap.
#[derive(Default)]
pub struct EmbeddedGraphBackend {
    tenants: RwLock<HashMap<String, TenantGraph>>,
}

#[async_trait]
impl GraphBackend for EmbeddedGraphBackend {
    async fn add_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<()> {
        // lock poison is unrecoverable
        let mut tenants = self.tenants.write().unwrap();
        tenants
            .entry(tenant_id.to_string())
            .or_default()
            .get_or_create(id);
        Ok(())
    }

    async fn add_edge(&self, tenant_id: &str, edge: GraphEdge) -> BackendResult<()> {
        let mut tenants = self.tenants.write().unwrap();
        let tenant = tenants.entry(tenant_id.to_string()).or_default();
        // Auto-create endpoints that weren't explicitly added via add_node,
        // matching MockGraphBackend::add_edge — it stores the edge
        // unconditionally without checking the node list first.
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
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_and_traverse_edge() {
        let backend = EmbeddedGraphBackend::default();
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
        let backend = EmbeddedGraphBackend::default();
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
        let backend = EmbeddedGraphBackend::default();
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
        let backend = EmbeddedGraphBackend::default();
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
}
