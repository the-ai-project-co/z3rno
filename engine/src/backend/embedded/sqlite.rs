//! Embedded, file-based `EngineBackend` implementation on top of `rusqlite`
//! (with its `bundled` feature, so no system SQLite install is required —
//! this is z3rno's zero-infra default). Single-writer: one file, one
//! connection, one lock.
//!
//! Locking: `rusqlite::Connection` is `!Sync`, so it's wrapped in
//! `std::sync::Mutex` rather than `tokio::sync::Mutex` — every method here
//! does its work synchronously (no `.await` while the lock is held), and a
//! std mutex is the right tool when the critical section never yields.
//!
//! Blocking: methods issue rusqlite calls directly on the calling task
//! instead of going through `tokio::task::spawn_blocking`. This is a small
//! embedded backend (local file, single writer, no network) — a `put`/`get`
//! against SQLite on local disk is a few microseconds, not a stall worth
//! shipping to a blocking-pool thread. It also means this file adds no
//! runtime dependency on `tokio` outside of tests. If this backend is later
//! pushed into a high-concurrency host, revisit with `spawn_blocking`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::super::error::BackendResult;
use super::super::relational::{EngineBackend, Record};
use super::conn;

/// Embedded SQLite-backed `EngineBackend`. Shares its connection with the
/// other embedded backends (`MemoryEngine::embedded` opens one and hands
/// each backend a clone of the `Arc`) — matches the single-writer,
/// zero-infra default this crate ships with, one file, one connection.
pub struct SqliteEngineBackend {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteEngineBackend {
    /// Opens (creating if missing) a SQLite file at `path`, on its own
    /// connection, and ensures the schema exists. For standalone use
    /// (tests, or any caller that doesn't need to share the file with the
    /// vector/graph backends) — `MemoryEngine::embedded` uses
    /// `with_connection` instead, so all three backends share one
    /// connection to the same file.
    pub fn open<P: AsRef<Path>>(path: P) -> BackendResult<Self> {
        Self::with_connection(conn::open(path)?)
    }

    /// Same as `open`, but against an existing shared connection (already
    /// pointed at a real file or `:memory:`) rather than opening its own.
    pub(crate) fn with_connection(conn: Arc<Mutex<Connection>>) -> BackendResult<Self> {
        {
            // lock poison is unrecoverable
            let c = conn.lock().unwrap();
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS records (
                    id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    data TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, id)
                );
                CREATE INDEX IF NOT EXISTS idx_records_tenant_kind
                    ON records (tenant_id, kind);",
            )
            .map_err(anyhow::Error::from)?;
        }
        Ok(Self { conn })
    }

    fn row_to_record(
        id: String,
        tenant_id: String,
        kind: String,
        data: String,
        created_at: String,
    ) -> anyhow::Result<Record> {
        Ok(Record {
            id: Uuid::parse_str(&id)?,
            tenant_id,
            kind,
            data: serde_json::from_str(&data)?,
            created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
        })
    }
}

#[async_trait]
impl EngineBackend for SqliteEngineBackend {
    async fn put(&self, record: Record) -> BackendResult<()> {
        let data = serde_json::to_string(&record.data).map_err(anyhow::Error::from)?;
        // lock poison is unrecoverable
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO records (id, tenant_id, kind, data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (tenant_id, id) DO UPDATE SET
                kind = excluded.kind,
                data = excluded.data,
                created_at = excluded.created_at",
            params![
                record.id.to_string(),
                record.tenant_id,
                record.kind,
                data,
                record.created_at.to_rfc3339(),
            ],
        )
        .map_err(anyhow::Error::from)?;
        Ok(())
    }

    async fn get(&self, tenant_id: &str, id: Uuid) -> BackendResult<Option<Record>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, tenant_id, kind, data, created_at FROM records
                 WHERE tenant_id = ?1 AND id = ?2",
                params![tenant_id, id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(anyhow::Error::from)?;
        match row {
            Some((id, tenant_id, kind, data, created_at)) => {
                let record = Self::row_to_record(id, tenant_id, kind, data, created_at)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "DELETE FROM records WHERE tenant_id = ?1 AND id = ?2",
                params![tenant_id, id.to_string()],
            )
            .map_err(anyhow::Error::from)?;
        Ok(affected > 0)
    }

    async fn list(&self, tenant_id: &str, kind: &str) -> BackendResult<Vec<Record>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, tenant_id, kind, data, created_at FROM records
                 WHERE tenant_id = ?1 AND kind = ?2",
            )
            .map_err(anyhow::Error::from)?;
        let rows = stmt
            .query_map(params![tenant_id, kind], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(anyhow::Error::from)?;

        let mut records = Vec::new();
        for row in rows {
            let (id, tenant_id, kind, data, created_at) = row.map_err(anyhow::Error::from)?;
            records.push(Self::row_to_record(id, tenant_id, kind, data, created_at)?);
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A SQLite file under the OS temp dir, unique per test run, cleaned up
    /// on drop. Avoids adding a `tempfile` dependency for this one use.
    struct TempDbPath(PathBuf);

    impl TempDbPath {
        fn new(name: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!("z3rno-sqlite-test-{name}-{}.db", Uuid::new_v4()));
            Self(path)
        }
    }

    impl Drop for TempDbPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn sample_record(tenant_id: &str) -> Record {
        Record {
            id: Uuid::new_v4(),
            tenant_id: tenant_id.to_string(),
            kind: "memory".into(),
            data: serde_json::json!({"content": "hello"}),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn put_then_get_round_trips_through_a_real_sqlite_file() {
        let db_path = TempDbPath::new("roundtrip");
        let backend = SqliteEngineBackend::open(&db_path.0).unwrap();
        let record = sample_record("tenant-a");

        backend.put(record.clone()).await.unwrap();
        let fetched = backend.get("tenant-a", record.id).await.unwrap();

        assert_eq!(fetched, Some(record));
    }

    #[tokio::test]
    async fn get_is_tenant_scoped() {
        let db_path = TempDbPath::new("tenant-scope");
        let backend = SqliteEngineBackend::open(&db_path.0).unwrap();
        let record = sample_record("tenant-a");
        backend.put(record.clone()).await.unwrap();

        assert!(backend.get("tenant-b", record.id).await.unwrap().is_none());
        assert_eq!(
            backend.get("tenant-a", record.id).await.unwrap(),
            Some(record)
        );
    }

    #[tokio::test]
    async fn delete_removes_only_the_matching_tenants_row() {
        let db_path = TempDbPath::new("delete");
        let backend = SqliteEngineBackend::open(&db_path.0).unwrap();
        let record = sample_record("tenant-a");
        backend.put(record.clone()).await.unwrap();

        // wrong tenant: nothing deleted, row still there
        assert!(!backend.delete("tenant-b", record.id).await.unwrap());
        assert!(backend.get("tenant-a", record.id).await.unwrap().is_some());

        assert!(backend.delete("tenant-a", record.id).await.unwrap());
        assert!(backend.get("tenant-a", record.id).await.unwrap().is_none());
        // deleting again reports nothing left to delete
        assert!(!backend.delete("tenant-a", record.id).await.unwrap());
    }

    #[tokio::test]
    async fn list_filters_by_tenant_and_kind() {
        let db_path = TempDbPath::new("list");
        let backend = SqliteEngineBackend::open(&db_path.0).unwrap();

        let a_memory = sample_record("tenant-a");
        let mut a_audit = sample_record("tenant-a");
        a_audit.kind = "audit".into();
        let b_memory = sample_record("tenant-b");

        backend.put(a_memory.clone()).await.unwrap();
        backend.put(a_audit.clone()).await.unwrap();
        backend.put(b_memory).await.unwrap();

        let results = backend.list("tenant-a", "memory").await.unwrap();
        assert_eq!(results, vec![a_memory]);
    }
}
