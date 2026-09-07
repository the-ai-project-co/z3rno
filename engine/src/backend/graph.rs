use async_trait::async_trait;
use uuid::Uuid;

use super::error::BackendResult;

/// A directed, named relationship between two node ids (e.g. an episodic
/// link or a semantic association — the domain layer decides what
/// relationship names mean, this trait just stores and traverses them).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub source: Uuid,
    pub target: Uuid,
    pub relationship: String,
}

/// Graph storage and one-hop traversal, tenant-scoped. Every backend —
/// embedded (in-process graph) or production (Apache AGE) — implements this
/// trait; callers depend only on it, never on a concrete backend.
#[async_trait]
pub trait GraphBackend: Send + Sync {
    async fn add_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<()>;
    async fn add_edge(&self, tenant_id: &str, edge: GraphEdge) -> BackendResult<()>;
    async fn neighbors(
        &self,
        tenant_id: &str,
        id: Uuid,
        relationship: Option<&str>,
    ) -> BackendResult<Vec<Uuid>>;
    async fn remove_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool>;
}
