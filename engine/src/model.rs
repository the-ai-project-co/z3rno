use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The current time at microsecond precision, the finest Postgres
/// `timestamptz` stores. Timestamps that are persisted are created with
/// this, so a record reads back exactly as it was written on every
/// backend (Linux clocks have nanosecond resolution).
pub fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

/// The four memory tiers z3rno organizes storage across.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Working,
    Episodic,
    Semantic,
    Procedural,
}

/// One stored memory: content, its tier, and whatever metadata the caller
/// attached. This is a single current value, not a version history — see
/// `crate::audit` for what "auditable" actually means here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub id: Uuid,
    pub tenant_id: String,
    pub tier: Tier,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn now_has_no_sub_microsecond_part() {
        assert_eq!(super::now().timestamp_subsec_nanos() % 1_000, 0);
    }
}
