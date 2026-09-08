//! The regression gate: runs the shared golden dataset
//! (`evals/fixtures/golden_v1.json`) end-to-end through a real
//! `MemoryEngine` and asserts real thresholds. This is a normal
//! `cargo test`, so once `evals/rust` is a workspace member it runs on
//! every PR via the existing CI `test` job — no separate workflow needed.
//!
//! Exercises all three headline verbs: `store` (seeding), `recall`
//! (scoring), and `forget` (the dedicated `forget_removes_memory_from_recall`
//! test below).

use std::collections::HashMap;
use std::time::Instant;

use uuid::Uuid;
use z3rno_engine::MemoryEngine;
use z3rno_evals::{faithfulness, latency_percentiles, mrr, recall_at_k, Dataset};
use z3rno_hash_embed::hash_embed;

fn fixture_path() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is evals/rust; the fixture lives at the repo-root
    // evals/fixtures/, shared across the Python/TypeScript/Rust harnesses.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden_v1.json")
}

async fn seed(engine: &MemoryEngine, dataset: &Dataset) -> HashMap<String, Uuid> {
    let mut ids = HashMap::new();
    for s in &dataset.seed {
        let memory = engine
            .store(
                &dataset.tenant_id,
                s.tier,
                s.content.clone(),
                Some(hash_embed(&s.content)),
                s.metadata.clone(),
                Vec::new(),
            )
            .await
            .expect("seed store should succeed");
        ids.insert(s.id.clone(), memory.id);
    }
    ids
}

// Measured baseline (from this test's own `println!`, run with
// `--nocapture`) against this specific 10-seed/6-query dataset, on the
// naive hashing-embedding pipeline: mean_recall@k=0.8333, mean_mrr=0.8333,
// mean_faithfulness=0.8125, p95=0.936ms. Thresholds below sit with real
// headroom under that baseline (~0.13 on recall/mrr, ~0.16 on
// faithfulness, ~200x on latency) so a minor future fixture edit (a
// reworded query, an added seed) doesn't flake CI, while still catching a
// real regression in store/recall wiring or the embedding.
const MIN_MEAN_RECALL_AT_K: f64 = 0.7;
const MIN_MEAN_MRR: f64 = 0.7;
const MIN_MEAN_FAITHFULNESS: f64 = 0.65;
const MAX_P95_LATENCY_MS: f64 = 200.0;

#[tokio::test]
async fn golden_dataset_regression_gate() {
    let dataset = Dataset::load(fixture_path()).expect("golden_v1.json should parse");

    let db_file = tempfile::NamedTempFile::new().unwrap();
    let engine = MemoryEngine::embedded(db_file.path()).unwrap();

    let seeded_ids = seed(&engine, &dataset).await;

    let mut recalls = Vec::new();
    let mut mrrs = Vec::new();
    let mut faithfulnesses = Vec::new();
    let mut latencies = Vec::new();

    for item in &dataset.items {
        let query_embedding = hash_embed(&item.query);

        let started = Instant::now();
        let results = engine
            .recall(&dataset.tenant_id, query_embedding, item.top_k)
            .await
            .expect("recall should succeed");
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        latencies.push(elapsed_ms);

        let retrieved_ids: Vec<Uuid> = results.iter().map(|m| m.id).collect();
        let expected_ids: Vec<Uuid> = item
            .expected_seed_ids
            .iter()
            .map(|fixture_id| {
                *seeded_ids.get(fixture_id).unwrap_or_else(|| {
                    panic!("item {} references unknown seed id {fixture_id}", item.id)
                })
            })
            .collect();

        let r = recall_at_k(&retrieved_ids, &expected_ids, item.top_k);
        let m = mrr(&retrieved_ids, &expected_ids);
        let recalled_context = results
            .iter()
            .map(|mem| mem.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let f = faithfulness(&item.expected_answer, &recalled_context);

        if !r.is_nan() {
            recalls.push(r);
        }
        if !m.is_nan() {
            mrrs.push(m);
        }
        if !f.is_nan() {
            faithfulnesses.push(f);
        }
    }

    let mean = |xs: &[f64]| -> f64 {
        if xs.is_empty() {
            f64::NAN
        } else {
            xs.iter().sum::<f64>() / xs.len() as f64
        }
    };
    let mean_recall = mean(&recalls);
    let mean_mrr = mean(&mrrs);
    let mean_faithfulness = mean(&faithfulnesses);
    let lat = latency_percentiles(&latencies);

    println!(
        "golden_v1 baseline: mean_recall@k={mean_recall:.4} mean_mrr={mean_mrr:.4} \
         mean_faithfulness={mean_faithfulness:.4} p50={:.3}ms p95={:.3}ms p99={:.3}ms mean_lat={:.3}ms",
        lat.p50, lat.p95, lat.p99, lat.mean
    );

    assert!(
        mean_recall >= MIN_MEAN_RECALL_AT_K,
        "mean recall@k {mean_recall} below floor {MIN_MEAN_RECALL_AT_K}"
    );
    assert!(
        mean_mrr >= MIN_MEAN_MRR,
        "mean mrr {mean_mrr} below floor {MIN_MEAN_MRR}"
    );
    assert!(
        mean_faithfulness >= MIN_MEAN_FAITHFULNESS,
        "mean faithfulness {mean_faithfulness} below floor {MIN_MEAN_FAITHFULNESS}"
    );
    assert!(
        lat.p95 <= MAX_P95_LATENCY_MS,
        "p95 latency {}ms above ceiling {MAX_P95_LATENCY_MS}ms",
        lat.p95
    );
}

#[tokio::test]
async fn forget_removes_memory_from_recall() {
    let dataset = Dataset::load(fixture_path()).expect("golden_v1.json should parse");

    let db_file = tempfile::NamedTempFile::new().unwrap();
    let engine = MemoryEngine::embedded(db_file.path()).unwrap();

    let seeded_ids = seed(&engine, &dataset).await;

    // q-002's only expected seed is m-004 ("run make deploy-staging...").
    // Forget it, then confirm the same query no longer recalls it.
    let target_fixture_id = "m-004";
    let target_id = *seeded_ids.get(target_fixture_id).unwrap();

    let proof = engine
        .forget(&dataset.tenant_id, target_id)
        .await
        .expect("forget should succeed");
    assert!(
        proof.is_some(),
        "forgetting a real memory should return a proof"
    );

    let item = dataset
        .items
        .iter()
        .find(|i| i.expected_seed_ids == vec![target_fixture_id.to_string()])
        .expect("fixture should have a single-target item for m-004");

    let results = engine
        .recall(&dataset.tenant_id, hash_embed(&item.query), item.top_k)
        .await
        .unwrap();
    let retrieved_ids: Vec<Uuid> = results.iter().map(|m| m.id).collect();

    assert_eq!(
        recall_at_k(&retrieved_ids, &[target_id], item.top_k),
        0.0,
        "forgotten memory should no longer be recallable"
    );

    // Forgetting again is a no-op, not an error (matches the engine's own
    // forget contract, exercised in cli/src/main.rs's own tests).
    let second = engine.forget(&dataset.tenant_id, target_id).await.unwrap();
    assert!(second.is_none());
}
