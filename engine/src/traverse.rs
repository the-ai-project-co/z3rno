//! Typed, directed links between memories and bounded multi-hop traversal
//! over them (`engine.advanced().link/unlink/traverse`).
//!
//! Every id returned is joined back through the relational store level by
//! level, so a graph node whose memory was forgotten (cleanup pending) is
//! neither returned nor expanded.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::{BackendError, BackendResult, EdgeDirection, ErrorCode, GraphEdge};
use crate::engine::Advanced;

pub const DEFAULT_MAX_DEPTH: usize = 2;
pub const MAX_DEPTH: usize = 5;
pub const DEFAULT_MAX_NODES: usize = 100;
pub const MAX_NODES: usize = 500;
const MAX_RELATIONSHIP: usize = 200;

/// A traversal request. Caps are clamped, never trusted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraverseQuery {
    #[serde(default)]
    pub direction: EdgeDirection,
    /// Relationship names to follow; empty follows every relationship.
    #[serde(default)]
    pub relationships: Vec<String>,
    /// Hops from the start (default 2, at most 5).
    #[serde(default)]
    pub max_depth: Option<usize>,
    /// Memories returned (default 100, at most 500).
    #[serde(default)]
    pub max_nodes: Option<usize>,
}

/// One memory reached by a traversal: how far from the start, and the
/// edge (with its real direction) it was first reached through.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraversalStep {
    pub id: Uuid,
    pub depth: usize,
    pub via: GraphEdge,
}

fn rejected(message: impl Into<String>) -> BackendError {
    BackendError::rejected(ErrorCode::InvalidInput, message)
}

pub(crate) fn check_relationship(relationship: &str) -> BackendResult<()> {
    if relationship.is_empty() || relationship.chars().count() > MAX_RELATIONSHIP {
        return Err(rejected(format!(
            "relationship must be 1..={MAX_RELATIONSHIP} characters"
        )));
    }
    Ok(())
}

impl Advanced<'_> {
    /// Adds `source -[relationship]-> target` between two stored memories.
    /// Idempotent: linking the same pair twice keeps one edge.
    pub async fn link(
        &self,
        tenant_id: &str,
        source: Uuid,
        target: Uuid,
        relationship: &str,
    ) -> BackendResult<()> {
        let relationship = relationship.trim();
        check_relationship(relationship)?;
        if source == target {
            return Err(rejected("a memory can't link to itself"));
        }
        let live = self
            .engine
            .live_memory_ids(tenant_id, vec![source, target])
            .await?;
        if live.len() != 2 {
            return Err(BackendError::NotFound(
                "both ends of a link must be stored memories".into(),
            ));
        }
        let graph = &self.engine.graph_backend;
        let existing = graph
            .edges(
                tenant_id,
                source,
                Some(relationship),
                EdgeDirection::Outgoing,
            )
            .await?;
        if existing.iter().any(|e| e.target == target) {
            return Ok(());
        }
        graph
            .add_edge(
                tenant_id,
                GraphEdge {
                    source,
                    target,
                    relationship: relationship.to_string(),
                },
            )
            .await
    }

    /// Removes `source -[relationship]-> target`; `true` if it existed.
    pub async fn unlink(
        &self,
        tenant_id: &str,
        source: Uuid,
        target: Uuid,
        relationship: &str,
    ) -> BackendResult<bool> {
        self.engine
            .graph_backend
            .remove_edge(
                tenant_id,
                &GraphEdge {
                    source,
                    target,
                    relationship: relationship.trim().to_string(),
                },
            )
            .await
    }

    /// Breadth-first traversal from `start`, nearest first. Each memory
    /// appears once, at the depth it was first reached. Returns nothing if
    /// `start` isn't a stored memory.
    pub async fn traverse(
        &self,
        tenant_id: &str,
        start: Uuid,
        query: &TraverseQuery,
    ) -> BackendResult<Vec<TraversalStep>> {
        let max_depth = query
            .max_depth
            .unwrap_or(DEFAULT_MAX_DEPTH)
            .clamp(1, MAX_DEPTH);
        let max_nodes = query
            .max_nodes
            .unwrap_or(DEFAULT_MAX_NODES)
            .clamp(1, MAX_NODES);
        let follow: HashSet<&str> = query.relationships.iter().map(|r| r.as_str()).collect();

        if self
            .engine
            .live_memory_ids(tenant_id, vec![start])
            .await?
            .is_empty()
        {
            return Ok(Vec::new());
        }

        let mut seen = HashSet::from([start]);
        let mut frontier = vec![start];
        let mut steps = Vec::new();
        for depth in 1..=max_depth {
            let mut reached: Vec<(Uuid, GraphEdge)> = Vec::new();
            for &node in &frontier {
                let mut edges = self
                    .engine
                    .graph_backend
                    .edges(tenant_id, node, None, query.direction)
                    .await?;
                // Stable order regardless of backend storage order.
                edges.sort_by(|a, b| {
                    (&a.relationship, a.source, a.target).cmp(&(
                        &b.relationship,
                        b.source,
                        b.target,
                    ))
                });
                for edge in edges {
                    if !follow.is_empty() && !follow.contains(edge.relationship.as_str()) {
                        continue;
                    }
                    let next = if edge.source == node {
                        edge.target
                    } else {
                        edge.source
                    };
                    if seen.insert(next) {
                        reached.push((next, edge));
                    }
                }
            }
            let ids = reached.iter().map(|(id, _)| *id).collect();
            let live: HashSet<Uuid> = self
                .engine
                .live_memory_ids(tenant_id, ids)
                .await?
                .into_iter()
                .collect();
            frontier.clear();
            for (id, via) in reached {
                if !live.contains(&id) {
                    continue;
                }
                if steps.len() == max_nodes {
                    return Ok(steps);
                }
                steps.push(TraversalStep { id, depth, via });
                frontier.push(id);
            }
            if frontier.is_empty() {
                break;
            }
        }
        Ok(steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::{MockEngineBackend, MockGraphBackend, MockVectorBackend};
    use crate::{BackendTier, MemoryEngine, Tier};
    use std::sync::Arc;

    fn mock() -> MemoryEngine {
        MemoryEngine::new(
            Arc::new(MockEngineBackend::default()),
            Arc::new(MockVectorBackend::default()),
            Arc::new(MockGraphBackend::default()),
            BackendTier::Embedded,
        )
    }

    async fn memory(e: &MemoryEngine, text: &str) -> Uuid {
        e.store(
            "t",
            Tier::Semantic,
            text.into(),
            None,
            serde_json::json!({}),
            vec![],
        )
        .await
        .unwrap()
        .id
    }

    async fn scenario(e: MemoryEngine) {
        // a -caused_by-> b -caused_by-> c ; d -mentions-> a
        let a = memory(&e, "a").await;
        let b = memory(&e, "b").await;
        let c = memory(&e, "c").await;
        let d = memory(&e, "d").await;
        let adv = e.advanced();
        adv.link("t", a, b, "caused_by").await.unwrap();
        adv.link("t", a, b, "caused_by").await.unwrap(); // idempotent
        adv.link("t", b, c, "caused_by").await.unwrap();
        adv.link("t", d, a, "mentions").await.unwrap();

        let out = adv
            .traverse("t", a, &TraverseQuery::default())
            .await
            .unwrap();
        let got: Vec<_> = out.iter().map(|s| (s.id, s.depth)).collect();
        assert_eq!(got, vec![(b, 1), (c, 2)]);

        let incoming = TraverseQuery {
            direction: EdgeDirection::Incoming,
            ..Default::default()
        };
        let back = adv.traverse("t", a, &incoming).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, d);
        assert_eq!((back[0].via.source, back[0].via.target), (d, a));

        let both = TraverseQuery {
            direction: EdgeDirection::Both,
            relationships: vec!["caused_by".into()],
            max_depth: Some(1),
            ..Default::default()
        };
        let ids: Vec<_> = adv
            .traverse("t", b, &both)
            .await
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a) && ids.contains(&c));

        let capped = TraverseQuery {
            direction: EdgeDirection::Both,
            max_nodes: Some(1),
            ..Default::default()
        };
        assert_eq!(adv.traverse("t", a, &capped).await.unwrap().len(), 1);

        // Forgetting b hides it and cuts the path through it.
        e.forget("t", b).await.unwrap();
        assert!(adv
            .traverse("t", a, &TraverseQuery::default())
            .await
            .unwrap()
            .is_empty());

        assert!(adv.unlink("t", d, a, "mentions").await.unwrap());
        assert!(!adv.unlink("t", d, a, "mentions").await.unwrap());
        assert!(adv.traverse("t", a, &incoming).await.unwrap().is_empty());

        // Other tenants see nothing.
        assert!(e
            .advanced()
            .traverse("other", c, &TraverseQuery::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn embedded_link_traverse_unlink() {
        let mut path = std::env::temp_dir();
        path.push(format!("z3rno-traverse-{}.db", Uuid::new_v4()));
        scenario(MemoryEngine::embedded(path).unwrap()).await;
    }

    #[tokio::test]
    async fn mock_link_traverse_unlink() {
        scenario(mock()).await;
    }

    #[tokio::test]
    #[ignore = "needs a Postgres+AGE instance (Z3RNO_TEST_POSTGRES_URL)"]
    async fn postgres_link_traverse_unlink() {
        let url = std::env::var("Z3RNO_TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:55433/z3rno_dev".into());
        scenario(MemoryEngine::postgres(&url).await.unwrap()).await;
    }

    #[tokio::test]
    async fn link_validates_its_inputs() {
        let e = mock();
        let a = memory(&e, "a").await;
        let adv = e.advanced();
        let missing = Uuid::new_v4();
        assert!(matches!(
            adv.link("t", a, missing, "x").await,
            Err(BackendError::NotFound(_))
        ));
        assert_eq!(
            adv.link("t", a, a, "x").await.unwrap_err().code(),
            Some(ErrorCode::InvalidInput)
        );
        assert_eq!(
            adv.link("t", a, missing, " ").await.unwrap_err().code(),
            Some(ErrorCode::InvalidInput)
        );
    }
}
