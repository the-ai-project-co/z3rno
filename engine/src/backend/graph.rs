use async_trait::async_trait;
use uuid::Uuid;

use super::error::BackendResult;

/// A directed, named relationship between two node ids (e.g. an episodic
/// link or a semantic association — the domain layer decides what
/// relationship names mean, this trait just stores and traverses them).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphEdge {
    pub source: Uuid,
    pub target: Uuid,
    pub relationship: String,
}

/// Which edges of a node to follow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeDirection {
    /// Edges that start at the node.
    #[default]
    Outgoing,
    /// Edges that end at the node.
    Incoming,
    /// Both.
    Both,
}

impl EdgeDirection {
    pub(crate) fn includes_outgoing(self) -> bool {
        matches!(self, EdgeDirection::Outgoing | EdgeDirection::Both)
    }

    pub(crate) fn includes_incoming(self) -> bool {
        matches!(self, EdgeDirection::Incoming | EdgeDirection::Both)
    }
}

/// Graph storage and one-hop traversal, tenant-scoped. Every backend —
/// embedded (in-process graph) or production (Apache AGE) — implements this
/// trait; callers depend only on it, never on a concrete backend.
#[async_trait]
pub trait GraphBackend: Send + Sync {
    async fn add_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<()>;
    async fn add_edge(&self, tenant_id: &str, edge: GraphEdge) -> BackendResult<()>;
    /// Edges touching `id` in `direction`, optionally filtered by
    /// relationship name, each with its real source and target.
    async fn edges(
        &self,
        tenant_id: &str,
        id: Uuid,
        relationship: Option<&str>,
        direction: EdgeDirection,
    ) -> BackendResult<Vec<GraphEdge>>;
    /// Removes every `source -[relationship]-> target` edge; `true` if
    /// there was one.
    async fn remove_edge(&self, tenant_id: &str, edge: &GraphEdge) -> BackendResult<bool>;
    async fn remove_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool>;

    /// Targets of `id`'s outgoing edges.
    async fn neighbors(
        &self,
        tenant_id: &str,
        id: Uuid,
        relationship: Option<&str>,
    ) -> BackendResult<Vec<Uuid>> {
        Ok(self
            .edges(tenant_id, id, relationship, EdgeDirection::Outgoing)
            .await?
            .into_iter()
            .map(|e| e.target)
            .collect())
    }
}
