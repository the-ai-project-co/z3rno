//! Decision-doc 0004's pluggable cache backend, retrofitted here from
//! scratch rather than ported: the old Python server had `redis[hiredis]`
//! as a *base* dependency, with `/v1/sessions` and the API-key
//! verification cache calling `_get_redis()` directly — the server
//! literally could not start without a Redis-compatible instance. That's
//! the one thing this interface exists to make optional.
//!
//! One trait, three call sites: `/v1/sessions` (server/src/routes/
//! sessions.rs), the auth verification cache (server/src/auth), and the
//! admin budget store (server/src/routes/admin.rs) — all just get/set/
//! delete a string value by key, optionally with a TTL, so one small
//! trait covers all three rather than three bespoke stores.
//!
//! `SqliteCacheBackend` is the default (decision-doc 0004: "flip default
//! to sqlite" from day one here, since there's no existing deployment to
//! stay backward-compatible with yet). `RedisCacheBackend` is opt-in,
//! behind the `redis-cache` Cargo feature — the server's full test suite
//! passes with zero external cache service running, using the default.

use async_trait::async_trait;
use std::time::Duration;

mod sqlite;
pub use sqlite::SqliteCacheBackend;

#[cfg(feature = "redis-cache")]
mod redis_backend;
#[cfg(feature = "redis-cache")]
pub use redis_backend::RedisCacheBackend;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache backend error: {0}")]
    Backend(#[from] anyhow::Error),
}

pub type CacheResult<T> = Result<T, CacheError>;

#[async_trait]
pub trait CacheBackend: Send + Sync {
    /// Reads a value. `Ok(None)` means the key doesn't exist or has
    /// expired — not an error.
    async fn get(&self, key: &str) -> CacheResult<Option<String>>;

    /// Writes a value, optionally with a TTL after which `get` should
    /// stop returning it. A `None` ttl means no expiry.
    async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> CacheResult<()>;

    /// Removes a key. Deleting a key that doesn't exist is not an error.
    async fn delete(&self, key: &str) -> CacheResult<()>;
}
