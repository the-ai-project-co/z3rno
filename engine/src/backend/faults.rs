//! Fault-injecting decorators over a real backend, for tests that need a
//! derived-store delete to fail on demand (the forget protocol's failure
//! paths). Every other call passes straight through to the wrapped backend.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use super::error::{BackendError, BackendResult};
use super::graph::{EdgeDirection, GraphBackend, GraphEdge};
use super::vector::{VectorBackend, VectorMatch};

/// Consumes one unit of a "fail the next N calls" budget, returning the
/// injected error while any budget remains.
fn take_fault(budget: &AtomicUsize, what: &str) -> BackendResult<()> {
    let mut remaining = budget.load(Ordering::SeqCst);
    while remaining > 0 {
        match budget.compare_exchange(remaining, remaining - 1, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => {
                return Err(BackendError::Storage(anyhow::anyhow!(
                    "injected {what} failure"
                )))
            }
            Err(current) => remaining = current,
        }
    }
    Ok(())
}

pub struct FaultyVector {
    inner: Arc<dyn VectorBackend>,
    failing_deletes: AtomicUsize,
}

impl FaultyVector {
    pub fn new(inner: Arc<dyn VectorBackend>) -> Self {
        Self {
            inner,
            failing_deletes: AtomicUsize::new(0),
        }
    }

    pub fn fail_next_deletes(&self, n: usize) {
        self.failing_deletes.store(n, Ordering::SeqCst);
    }
}

#[async_trait]
impl VectorBackend for FaultyVector {
    async fn upsert(&self, tenant_id: &str, id: Uuid, embedding: Vec<f32>) -> BackendResult<()> {
        self.inner.upsert(tenant_id, id, embedding).await
    }

    async fn search(
        &self,
        tenant_id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> BackendResult<Vec<VectorMatch>> {
        self.inner.search(tenant_id, query, k).await
    }

    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        take_fault(&self.failing_deletes, "vector delete")?;
        self.inner.delete(tenant_id, id).await
    }
}

pub struct FaultyGraph {
    inner: Arc<dyn GraphBackend>,
    failing_removes: AtomicUsize,
}

impl FaultyGraph {
    pub fn new(inner: Arc<dyn GraphBackend>) -> Self {
        Self {
            inner,
            failing_removes: AtomicUsize::new(0),
        }
    }

    pub fn fail_next_removes(&self, n: usize) {
        self.failing_removes.store(n, Ordering::SeqCst);
    }
}

#[async_trait]
impl GraphBackend for FaultyGraph {
    async fn add_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<()> {
        self.inner.add_node(tenant_id, id).await
    }

    async fn add_edge(&self, tenant_id: &str, edge: GraphEdge) -> BackendResult<()> {
        self.inner.add_edge(tenant_id, edge).await
    }

    async fn edges(
        &self,
        tenant_id: &str,
        id: Uuid,
        relationship: Option<&str>,
        direction: EdgeDirection,
    ) -> BackendResult<Vec<GraphEdge>> {
        self.inner
            .edges(tenant_id, id, relationship, direction)
            .await
    }

    async fn remove_edge(&self, tenant_id: &str, edge: &GraphEdge) -> BackendResult<bool> {
        self.inner.remove_edge(tenant_id, edge).await
    }

    async fn remove_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        take_fault(&self.failing_removes, "graph remove")?;
        self.inner.remove_node(tenant_id, id).await
    }
}
