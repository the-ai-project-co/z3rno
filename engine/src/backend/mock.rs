//! In-memory mock implementations of the three backend traits, behind the
//! `testing` feature. Used by this crate's own tests and by downstream
//! sub-slices (0003.5/0003.6) that need a backend without touching disk.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use uuid::Uuid;

use super::error::BackendResult;
use super::graph::{GraphBackend, GraphEdge};
use super::relational::{EngineBackend, Record};
use super::vector::{VectorBackend, VectorMatch};

fn tenant_key(tenant_id: &str, id: Uuid) -> String {
    format!("{tenant_id}:{id}")
}

#[derive(Default)]
pub struct MockEngineBackend {
    records: Mutex<HashMap<String, Record>>,
}

#[async_trait]
impl EngineBackend for MockEngineBackend {
    async fn put(&self, record: Record) -> BackendResult<()> {
        let key = tenant_key(&record.tenant_id, record.id);
        // lock poison is unrecoverable
        self.records.lock().unwrap().insert(key, record);
        Ok(())
    }

    async fn get(&self, tenant_id: &str, id: Uuid) -> BackendResult<Option<Record>> {
        let key = tenant_key(tenant_id, id);
        Ok(self.records.lock().unwrap().get(&key).cloned())
    }

    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let key = tenant_key(tenant_id, id);
        Ok(self.records.lock().unwrap().remove(&key).is_some())
    }

    async fn list(&self, tenant_id: &str, kind: &str) -> BackendResult<Vec<Record>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.tenant_id == tenant_id && r.kind == kind)
            .cloned()
            .collect())
    }
}

#[derive(Default)]
pub struct MockVectorBackend {
    vectors: Mutex<HashMap<String, Vec<f32>>>,
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[async_trait]
impl VectorBackend for MockVectorBackend {
    async fn upsert(&self, tenant_id: &str, id: Uuid, embedding: Vec<f32>) -> BackendResult<()> {
        let key = tenant_key(tenant_id, id);
        self.vectors.lock().unwrap().insert(key, embedding);
        Ok(())
    }

    async fn search(
        &self,
        tenant_id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> BackendResult<Vec<VectorMatch>> {
        let prefix = format!("{tenant_id}:");
        let vectors = self.vectors.lock().unwrap();
        let mut matches: Vec<VectorMatch> = vectors
            .iter()
            .filter_map(|(key, embedding)| {
                let id_str = key.strip_prefix(&prefix)?;
                let id = Uuid::parse_str(id_str).ok()?;
                Some(VectorMatch {
                    id,
                    score: cosine_similarity(&query, embedding),
                })
            })
            .collect();
        matches.sort_by(|a, b| b.score.total_cmp(&a.score));
        matches.truncate(k);
        Ok(matches)
    }

    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let key = tenant_key(tenant_id, id);
        Ok(self.vectors.lock().unwrap().remove(&key).is_some())
    }
}

#[derive(Default)]
pub struct MockGraphBackend {
    nodes: Mutex<HashMap<String, Vec<Uuid>>>,
    edges: Mutex<HashMap<String, Vec<GraphEdge>>>,
}

#[async_trait]
impl GraphBackend for MockGraphBackend {
    async fn add_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<()> {
        let entry = self
            .nodes
            .lock()
            .unwrap()
            .entry(tenant_id.to_string())
            .or_default()
            .clone();
        if !entry.contains(&id) {
            self.nodes
                .lock()
                .unwrap()
                .get_mut(tenant_id)
                .unwrap()
                .push(id);
        }
        Ok(())
    }

    async fn add_edge(&self, tenant_id: &str, edge: GraphEdge) -> BackendResult<()> {
        self.edges
            .lock()
            .unwrap()
            .entry(tenant_id.to_string())
            .or_default()
            .push(edge);
        Ok(())
    }

    async fn neighbors(
        &self,
        tenant_id: &str,
        id: Uuid,
        relationship: Option<&str>,
    ) -> BackendResult<Vec<Uuid>> {
        let edges = self.edges.lock().unwrap();
        let Some(tenant_edges) = edges.get(tenant_id) else {
            return Ok(Vec::new());
        };
        Ok(tenant_edges
            .iter()
            .filter(|e| e.source == id && relationship.is_none_or(|r| r == e.relationship))
            .map(|e| e.target)
            .collect())
    }

    async fn remove_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let mut nodes = self.nodes.lock().unwrap();
        let Some(tenant_nodes) = nodes.get_mut(tenant_id) else {
            return Ok(false);
        };
        let before = tenant_nodes.len();
        tenant_nodes.retain(|n| *n != id);
        let removed = tenant_nodes.len() != before;
        drop(nodes);
        if removed {
            if let Some(tenant_edges) = self.edges.lock().unwrap().get_mut(tenant_id) {
                tenant_edges.retain(|e| e.source != id && e.target != id);
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::relational::Record;
    use chrono::Utc;

    #[tokio::test]
    async fn engine_backend_put_get_delete() {
        let backend = MockEngineBackend::default();
        let id = Uuid::new_v4();
        let record = Record {
            id,
            tenant_id: "tenant-a".into(),
            kind: "memory".into(),
            data: serde_json::json!({"content": "hello"}),
            created_at: Utc::now(),
        };
        backend.put(record.clone()).await.unwrap();

        let fetched = backend.get("tenant-a", id).await.unwrap();
        assert_eq!(fetched, Some(record));

        assert!(backend.get("tenant-b", id).await.unwrap().is_none());

        assert!(backend.delete("tenant-a", id).await.unwrap());
        assert!(backend.get("tenant-a", id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn vector_backend_search_returns_nearest_neighbor() {
        let backend = MockVectorBackend::default();
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

        let results = backend
            .search("tenant-a", vec![0.9, 0.1, 0.0], 1)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, a);
    }

    #[tokio::test]
    async fn graph_backend_add_and_traverse_edge() {
        let backend = MockGraphBackend::default();
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
    }
}
