//! The default `CacheBackend`: a single SQLite file, no external service.
//!
//! Same tradeoff `engine::backend::embedded::SqliteEngineBackend` already
//! documents and makes: a plain `std::sync::Mutex<Connection>` with sync
//! `rusqlite` calls inline in `async fn`, not `spawn_blocking` — fine at
//! this backend's actual concurrency (a cache, not a high-throughput
//! store), revisit if that stops being true.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::{params, Connection};

use super::{CacheBackend, CacheError, CacheResult};

pub struct SqliteCacheBackend {
    conn: Mutex<Connection>,
}

impl SqliteCacheBackend {
    pub fn open<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// An in-memory cache, mainly for tests — behaves identically to the
    /// file-backed variant, just not durable across a restart.
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init_schema(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cache_entries (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                expires_at INTEGER
            );",
        )?;
        Ok(())
    }

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }
}

#[async_trait]
impl CacheBackend for SqliteCacheBackend {
    async fn get(&self, key: &str) -> CacheResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(String, Option<i64>)> = conn
            .query_row(
                "SELECT value, expires_at FROM cache_entries WHERE key = ?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();

        match row {
            None => Ok(None),
            Some((_value, Some(expires_at))) if expires_at <= Self::now_secs() => {
                // Lazily evict: expired entries are deleted on read rather
                // than by a background sweep, since nothing here needs
                // exact-on-expiry eviction, only "never return stale data".
                let _ = conn.execute("DELETE FROM cache_entries WHERE key = ?1", params![key]);
                Ok(None)
            }
            Some((value, _)) => Ok(Some(value)),
        }
    }

    async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> CacheResult<()> {
        let expires_at = ttl.map(|d| Self::now_secs() + d.as_secs() as i64);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO cache_entries (key, value, expires_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
            params![key, value, expires_at],
        )
        .map_err(|e| CacheError::Backend(e.into()))?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> CacheResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM cache_entries WHERE key = ?1", params![key])
            .map_err(|e| CacheError::Backend(e.into()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_then_get_round_trips() {
        let cache = SqliteCacheBackend::open_in_memory().unwrap();
        cache.set("k", "v".to_string(), None).await.unwrap();
        assert_eq!(cache.get("k").await.unwrap(), Some("v".to_string()));
    }

    #[tokio::test]
    async fn missing_key_is_none_not_error() {
        let cache = SqliteCacheBackend::open_in_memory().unwrap();
        assert_eq!(cache.get("nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn expired_entry_is_evicted_on_read() {
        let cache = SqliteCacheBackend::open_in_memory().unwrap();
        // A zero-second TTL sets expires_at == now_secs() at write time,
        // and the read-time check is `expires_at <= now_secs()` — already
        // expired the moment it's read, no sleep needed to observe it.
        cache
            .set("k", "v".to_string(), Some(Duration::from_secs(0)))
            .await
            .unwrap();
        assert_eq!(cache.get("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_removes_the_key() {
        let cache = SqliteCacheBackend::open_in_memory().unwrap();
        cache.set("k", "v".to_string(), None).await.unwrap();
        cache.delete("k").await.unwrap();
        assert_eq!(cache.get("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn set_overwrites_existing_key() {
        let cache = SqliteCacheBackend::open_in_memory().unwrap();
        cache.set("k", "v1".to_string(), None).await.unwrap();
        cache.set("k", "v2".to_string(), None).await.unwrap();
        assert_eq!(cache.get("k").await.unwrap(), Some("v2".to_string()));
    }
}
