//! Opt-in Redis/Valkey `CacheBackend` — behind the `redis-cache` Cargo
//! feature, never a default or a hard requirement. Mirrors the old
//! Python server's actual Redis usage (`SET`/`GET`/`EXPIRE`, string
//! values) without the `_get_redis()`-with-no-fallback problem: this
//! type only exists if an operator explicitly built with the feature
//! and explicitly configured a Redis URL.

use std::time::Duration;

use async_trait::async_trait;
use redis::AsyncCommands;

use super::{CacheBackend, CacheError, CacheResult};

pub struct RedisCacheBackend {
    client: redis::Client,
}

impl RedisCacheBackend {
    pub fn connect(redis_url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl CacheBackend for RedisCacheBackend {
    async fn get(&self, key: &str) -> CacheResult<Option<String>> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError::Backend(e.into()))?;
        conn.get(key).await.map_err(|e| CacheError::Backend(e.into()))
    }

    async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> CacheResult<()> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError::Backend(e.into()))?;
        match ttl {
            Some(d) => {
                let _: () = conn
                    .set_ex(key, value, d.as_secs().max(1))
                    .await
                    .map_err(|e| CacheError::Backend(e.into()))?;
            }
            None => {
                let _: () = conn.set(key, value).await.map_err(|e| CacheError::Backend(e.into()))?;
            }
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> CacheResult<()> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError::Backend(e.into()))?;
        let _: () = conn.del(key).await.map_err(|e| CacheError::Backend(e.into()))?;
        Ok(())
    }
}

// No tests here requiring a live Redis instance — this crate's default
// test suite (matching 0005.5's acceptance criterion) runs with zero
// external services. If a live Redis integration test is ever added, it
// should follow the postgres_end_to_end.rs pattern in z3rno-engine:
// #[ignore]-gated, run explicitly against a real instance.
