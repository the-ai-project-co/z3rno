//! Loader for the shared golden dataset (`evals/fixtures/golden_v1.json`),
//! shaped to match that fixture's JSON exactly. `Seed::tier` deserializes
//! straight into `z3rno_engine::Tier` — the fixture's tier strings
//! ("working"/"episodic"/"semantic"/"procedural") are already
//! `Tier`'s own `serde(rename_all = "snake_case")` spelling, so there's no
//! separate parsing step to keep in sync.

use serde::Deserialize;
use z3rno_engine::Tier;

#[derive(Debug, Clone, Deserialize)]
pub struct Dataset {
    pub version: u32,
    pub name: String,
    pub description: String,
    pub tenant_id: String,
    pub seed: Vec<Seed>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Seed {
    pub id: String,
    pub tier: Tier,
    pub content: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub id: String,
    pub query: String,
    pub expected_seed_ids: Vec<String>,
    pub expected_answer: String,
    pub top_k: usize,
    pub latency_budget_ms: u64,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Dataset {
    /// Loads and parses a golden dataset fixture from `path`.
    pub fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Dataset> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }
}
