//! Slice 0004 acceptance: `MemoryEngine::postgres()` end-to-end against a
//! real Postgres+pgvector+AGE instance — the same store/recall/forget/
//! audit contract the embedded backend proves in `end_to_end.rs`, now
//! against the production backend. Requires the dev instance from
//! `engine/src/backend/postgres/docker-compose.dev.yml`.

use serde_json::json;
use uuid::Uuid;
use z3rno_engine::{BackendTier, MemoryEngine, Tier};

fn dev_database_url() -> String {
    std::env::var("Z3RNO_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:55433/z3rno_dev".into())
}

fn unique_tenant() -> String {
    format!("tenant-{}", Uuid::new_v4())
}

// Matches EMBEDDING_DIMS in backend::postgres::vector — the production
// vector table's column is a fixed vector(384).
const EMBEDDING_DIMS: usize = 384;

fn one_hot(index: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; EMBEDDING_DIMS];
    v[index] = 1.0;
    v
}

#[tokio::test]
#[ignore = "requires a running Postgres — see engine/src/backend/postgres/docker-compose.dev.yml"]
async fn postgres_engine_is_tagged_with_the_production_tier() {
    let engine = MemoryEngine::postgres(&dev_database_url()).await.unwrap();
    assert_eq!(engine.tier(), BackendTier::Postgres);
}

#[tokio::test]
#[ignore = "requires a running Postgres — see engine/src/backend/postgres/docker-compose.dev.yml"]
async fn store_recall_forget_against_the_real_production_backend() {
    let engine = MemoryEngine::postgres(&dev_database_url()).await.unwrap();
    let tenant = unique_tenant();

    let stored = engine
        .store(
            &tenant,
            Tier::Semantic,
            "the user's manager is named Priya".to_string(),
            Some(one_hot(0)),
            json!({}),
            Vec::new(),
        )
        .await
        .unwrap();

    let results = engine.recall(&tenant, one_hot(0), 1).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, stored.id);

    let proof = engine.forget(&tenant, stored.id).await.unwrap();
    assert!(proof.is_some());

    let results = engine.recall(&tenant, one_hot(0), 10).await.unwrap();
    assert!(results.is_empty());

    let audit = engine.advanced().audit(&tenant).await.unwrap();
    assert_eq!(audit.len(), 2); // Store, then Forget
    for event in &audit {
        assert!(event.is_self_consistent());
    }
}

#[tokio::test]
#[ignore = "requires a running Postgres — see engine/src/backend/postgres/docker-compose.dev.yml"]
async fn postgres_refuses_to_start_against_an_unreachable_database() {
    // 0004.4: a provisioning/connection failure must surface as an error
    // from the constructor itself, never a partially-working engine.
    let result = MemoryEngine::postgres("postgres://postgres:postgres@localhost:1/nope").await;
    assert!(result.is_err());
}
