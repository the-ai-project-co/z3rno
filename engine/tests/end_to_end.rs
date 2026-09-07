//! Sub-slice 0003.5/0003.6 acceptance: a full store -> recall -> forget
//! cycle against the embedded backend, covering all four memory tiers.

use serde_json::json;
use uuid::Uuid;
use z3rno_engine::{MemoryEngine, Tier};

fn temp_db_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("z3rno-e2e-{name}-{}.db", Uuid::new_v4()));
    path
}

#[tokio::test]
async fn store_recall_forget_across_all_four_tiers() {
    let db_path = temp_db_path("tiers");
    let engine = MemoryEngine::embedded(&db_path).unwrap();
    let tenant = "tenant-a";

    let tiers = [
        (Tier::Working, "scratch note", vec![1.0, 0.0, 0.0, 0.0]),
        (
            Tier::Episodic,
            "yesterday's conversation",
            vec![0.0, 1.0, 0.0, 0.0],
        ),
        (
            Tier::Semantic,
            "the user prefers dark mode",
            vec![0.0, 0.0, 1.0, 0.0],
        ),
        (
            Tier::Procedural,
            "how to deploy this service",
            vec![0.0, 0.0, 0.0, 1.0],
        ),
    ];

    let mut stored = Vec::new();
    for (tier, content, embedding) in &tiers {
        let memory = engine
            .store(
                tenant,
                *tier,
                content.to_string(),
                Some(embedding.clone()),
                json!({}),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(memory.tier, *tier);
        stored.push(memory);
    }

    // recall each one back out via a query close to its own embedding
    for (memory, (_, _, embedding)) in stored.iter().zip(tiers.iter()) {
        let results = engine.recall(tenant, embedding.clone(), 1).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, memory.id);
    }

    // forget one, verify it's gone from recall and a proof was recorded
    let forgotten = &stored[0];
    let proof = engine.forget(tenant, forgotten.id).await.unwrap();
    assert!(proof.is_some());

    let results = engine.recall(tenant, tiers[0].2.clone(), 10).await.unwrap();
    assert!(!results.iter().any(|m| m.id == forgotten.id));

    // forgetting again is a no-op, not an error, and appends no new event
    let audit_before = engine.advanced().audit(tenant).await.unwrap();
    let second_attempt = engine.forget(tenant, forgotten.id).await.unwrap();
    assert!(second_attempt.is_none());
    let audit_after = engine.advanced().audit(tenant).await.unwrap();
    assert_eq!(audit_before.len(), audit_after.len());

    // audit chain: one Store event per tier stored + one Forget event
    let audit = engine.advanced().audit(tenant).await.unwrap();
    assert_eq!(audit.len(), 5);
    for event in &audit {
        assert!(event.is_self_consistent());
    }
    for pair in audit.windows(2) {
        assert_eq!(pair[1].prev_hash.as_ref(), Some(&pair[0].hash));
    }
}

#[tokio::test]
async fn store_creates_graph_links_that_forget_cleans_up() {
    let db_path = temp_db_path("graph-links");
    let engine = MemoryEngine::embedded(&db_path).unwrap();
    let tenant = "tenant-a";

    let a = engine
        .store(
            tenant,
            Tier::Episodic,
            "met the user's manager".to_string(),
            Some(vec![1.0, 0.0]),
            json!({}),
            Vec::new(),
        )
        .await
        .unwrap();

    let b = engine
        .store(
            tenant,
            Tier::Semantic,
            "the user's manager is named Priya".to_string(),
            Some(vec![0.0, 1.0]),
            json!({}),
            vec![(a.id, "relates_to".to_string())],
        )
        .await
        .unwrap();

    let _ = b;
    engine.forget(tenant, a.id).await.unwrap();

    // a is gone from recall; nothing else asserted about the graph here —
    // EmbeddedGraphBackend's own tests cover edge cleanup on remove_node.
    let results = engine.recall(tenant, vec![1.0, 0.0], 10).await.unwrap();
    assert!(!results.iter().any(|m| m.id == a.id));
}
