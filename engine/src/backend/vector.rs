use async_trait::async_trait;
use uuid::Uuid;

use super::error::BackendResult;

/// One similarity-search hit: the id it was stored under, and a similarity
/// score (higher is more similar — backends normalize their own native
/// distance metric to this before returning).
#[derive(Debug, Clone, PartialEq)]
pub struct VectorMatch {
    pub id: Uuid,
    pub score: f32,
}

/// Vector storage and similarity search, tenant-scoped. Every backend —
/// embedded (in-process index) or production (pgvector) — implements this
/// trait; callers depend only on it, never on a concrete backend.
#[async_trait]
pub trait VectorBackend: Send + Sync {
    async fn upsert(&self, tenant_id: &str, id: Uuid, embedding: Vec<f32>) -> BackendResult<()>;
    async fn search(
        &self,
        tenant_id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> BackendResult<Vec<VectorMatch>>;
    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool>;
}
