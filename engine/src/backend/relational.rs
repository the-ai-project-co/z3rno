use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::error::BackendResult;

/// A tenant-scoped, JSON-payload record — metadata, an audit event, or
/// anything else the domain layer (memory tiers, audit log) needs to persist
/// relationally. `kind` distinguishes record types within one tenant; the
/// domain layer owns what kinds exist, this trait doesn't know or care.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: Uuid,
    pub tenant_id: String,
    pub kind: String,
    pub data: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// The relational half of memory storage: metadata, audit log, tenant/agent
/// records. Every backend — embedded (SQLite) or production (Postgres) —
/// implements this trait; callers depend only on it, never on a concrete
/// backend.
#[async_trait]
pub trait EngineBackend: Send + Sync {
    async fn put(&self, record: Record) -> BackendResult<()>;
    async fn get(&self, tenant_id: &str, id: Uuid) -> BackendResult<Option<Record>>;
    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool>;
    async fn list(&self, tenant_id: &str, kind: &str) -> BackendResult<Vec<Record>>;
}
