//! Postgres `VectorBackend`: pgvector-backed similarity search, tenant-
//! scoped via RLS (see `provision.rs`'s module docs for why `tenant_scoped_tx`
//! is fail-closed by default).
//!
//! ## Binding `Vec<f32>` as a pgvector `vector(384)` parameter
//!
//! `sqlx`'s Postgres driver has no built-in mapping for pgvector's `vector`
//! wire type. The `pgvector` crate (crates.io), built with its `sqlx`
//! feature, provides a `Vector` newtype with the right `Encode`/`Decode`
//! impls, so embeddings bind directly as `$1` with no manual wire-format or
//! text-literal handling — used here instead of the string-literal
//! (`'[1,2,3]'::vector`) fallback.
//!
//! ## Index: HNSW, with an `ivfflat` fallback
//!
//! HNSW (`vector_cosine_ops`) needs pgvector 0.5+; this file doesn't hard-
//! code a version check against `pg_extension.extversion` because the
//! failure mode is directly observable — attempt the `CREATE INDEX ...
//! USING hnsw`, and if the access method doesn't exist, fall back to
//! `ivfflat` (`lists = 100`, a reasonable default for small/medium tenant
//! corpora). The dev instance this was built and tested against runs
//! pgvector 0.8.6, so the HNSW path is what actually executes.
//!
//! ponytail: a failed HNSW attempt for a reason *other* than "access method
//! unsupported" (e.g. a transient connection error) also falls through to
//! the ivfflat attempt rather than surfacing the original error. Acceptable
//! here since schema setup runs once at construction against a trusted
//! connection; upgrade path if this ever hides a real failure: inspect the
//! `sqlx::Error` for the specific "access method \"hnsw\" does not exist"
//! message before deciding to fall back.
//!
//! ## Score conversion
//!
//! pgvector's `<=>` operator returns cosine *distance* in `[0, 2]` (0 =
//! identical, 2 = opposite); the trait's `VectorMatch::score` must be a
//! *similarity* where higher is more similar. Converted with `1.0 -
//! distance`, which is valid (not just monotonic) precisely because `<=>`'s
//! range is bounded to `[0, 2]` — same idea as the embedded backend's `1.0 /
//! (1.0 + distance)`, different formula because that backend's `DistCosine`
//! has no such documented bound.

use async_trait::async_trait;
use pgvector::Vector as PgVector;
use sqlx::PgPool;
use uuid::Uuid;

use super::provision::tenant_scoped_tx;
use crate::backend::error::BackendResult;
use crate::backend::vector::{VectorBackend, VectorMatch};

/// Fixed embedding width — matches the slice 0002 spike and the embedded
/// backend's tests. z3rno has no real embedding model wired in yet.
const EMBEDDING_DIMS: usize = 384;

pub struct PostgresVectorBackend {
    pool: PgPool,
}

/// Arbitrary but fixed `pg_advisory_lock` key, distinct from
/// `relational.rs`'s own schema-init lock key — see there for why this
/// exists: even `CREATE TABLE IF NOT EXISTS` has a race window under true
/// concurrency (confirmed live, as `duplicate key value violates unique
/// constraint "pg_type_typname_nsp_index"` — the implicit row type Postgres
/// creates alongside a table), so schema-init needs the whole sequence
/// serialized against concurrent callers, not just individual statements
/// made idempotent.
const SCHEMA_INIT_LOCK_KEY: i64 = 0x7a33_726e_6f5f_7632; // "z3rno_v2" in hex-ish

impl PostgresVectorBackend {
    /// Opens on an already-connected pool and idempotently provisions the
    /// `vectors` table, its RLS policy, and its similarity index. Assumes
    /// `provision::ensure_extensions` has already run (the `vector` type
    /// must exist before `CREATE TABLE` can reference it). Safe to call
    /// concurrently (multiple `z3rno-server` replicas booting against the
    /// same shared Postgres) — see [`SCHEMA_INIT_LOCK_KEY`].
    pub async fn new(pool: PgPool) -> BackendResult<Self> {
        let mut conn = pool.acquire().await.map_err(anyhow::Error::from)?;

        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(SCHEMA_INIT_LOCK_KEY)
            .execute(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;

        let init_result = ensure_schema(&mut conn).await;

        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(SCHEMA_INIT_LOCK_KEY)
            .execute(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;

        init_result?;
        drop(conn);
        Ok(Self { pool })
    }
}

async fn ensure_schema(conn: &mut sqlx::PgConnection) -> BackendResult<()> {
    // `public.` is load-bearing here, not decoration — see the matching
    // comment in `relational.rs`'s `init_schema`: every connection this
    // pool hands out has `ag_catalog` first on its search_path, and an
    // unqualified `CREATE TABLE vectors` would land there instead of
    // `public` (confirmed live). Everything else below stays unqualified
    // on purpose — it resolves an *existing* object by search-path lookup,
    // which correctly finds `public.vectors` once it's actually there.
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS public.vectors (
            tenant_id TEXT NOT NULL,
            id UUID NOT NULL,
            embedding vector({EMBEDDING_DIMS}) NOT NULL,
            PRIMARY KEY (tenant_id, id)
        )"
    ))
    .execute(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;

    sqlx::query("ALTER TABLE vectors ENABLE ROW LEVEL SECURITY")
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;
    sqlx::query("ALTER TABLE vectors FORCE ROW LEVEL SECURITY")
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

    // CREATE POLICY has no IF NOT EXISTS — swallow duplicate_object instead
    // (belt-and-suspenders alongside the advisory lock above, which is what
    // actually makes this safe under real concurrency).
    sqlx::query(
        "DO $$
         BEGIN
             CREATE POLICY vectors_tenant_isolation ON vectors
                 FOR ALL
                 USING (tenant_id = current_setting('app.current_tenant_id', true))
                 WITH CHECK (tenant_id = current_setting('app.current_tenant_id', true));
         EXCEPTION
             WHEN duplicate_object THEN NULL;
         END
         $$",
    )
    .execute(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;

    let hnsw = sqlx::query(
        "CREATE INDEX IF NOT EXISTS vectors_embedding_hnsw_idx \
         ON vectors USING hnsw (embedding vector_cosine_ops)",
    )
    .execute(&mut *conn)
    .await;
    if hnsw.is_err() {
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS vectors_embedding_ivfflat_idx \
             ON vectors USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100)",
        )
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;
    }

    Ok(())
}

#[async_trait]
impl VectorBackend for PostgresVectorBackend {
    async fn upsert(&self, tenant_id: &str, id: Uuid, embedding: Vec<f32>) -> BackendResult<()> {
        let mut tx = tenant_scoped_tx(&self.pool, tenant_id).await?;
        sqlx::query(
            "INSERT INTO vectors (tenant_id, id, embedding) VALUES ($1, $2, $3)
             ON CONFLICT (tenant_id, id) DO UPDATE SET embedding = excluded.embedding",
        )
        .bind(tenant_id)
        .bind(id)
        .bind(PgVector::from(embedding))
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        Ok(())
    }

    async fn search(
        &self,
        tenant_id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> BackendResult<Vec<VectorMatch>> {
        let mut tx = tenant_scoped_tx(&self.pool, tenant_id).await?;
        let rows: Vec<(Uuid, f64)> = sqlx::query_as(
            "SELECT id, embedding <=> $1 AS distance FROM vectors \
             WHERE tenant_id = $2 \
             ORDER BY embedding <=> $1 LIMIT $3",
        )
        .bind(PgVector::from(query))
        .bind(tenant_id)
        .bind(k as i64)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
        tx.commit().await.map_err(anyhow::Error::from)?;

        Ok(rows
            .into_iter()
            .map(|(id, distance)| VectorMatch {
                id,
                score: 1.0 - distance as f32,
            })
            .collect())
    }

    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let mut tx = tenant_scoped_tx(&self.pool, tenant_id).await?;
        let result = sqlx::query("DELETE FROM vectors WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::postgres::provision::{connect, ensure_extensions};

    // See relational.rs's test module for why each test opens its own pool
    // (a `static`-shared `PgPool` doesn't survive across `#[tokio::test]`'s
    // independent per-test runtimes) — confirmed stable at full default
    // test parallelism, no special invocation needed.

    fn dev_database_url() -> String {
        std::env::var("Z3RNO_TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:55433/z3rno_dev".into())
    }

    /// Unique per test run so parallel test runs (this suite, plus the
    /// relational and graph engineers' suites against the same live
    /// instance) can never collide on tenant id, even though this file
    /// owns its own `vectors` table.
    fn unique_tenant() -> String {
        format!("tenant-{}", Uuid::new_v4())
    }

    async fn backend() -> PostgresVectorBackend {
        let pool = connect(&dev_database_url()).await.unwrap();
        ensure_extensions(&pool).await.unwrap();
        PostgresVectorBackend::new(pool).await.unwrap()
    }

    /// Postgres superusers bypass RLS unconditionally — the shared dev
    /// instance's default credentials (`postgres/postgres`, used by
    /// [`backend`]) are superuser, so a search across two tenants through
    /// that connection can't actually prove isolation holds, it just
    /// doesn't happen to trip over the gap. `z3rno_app` (provisioned once
    /// at container init, see `postgres/initdb/01-app-role.sql`) is a real
    /// non-superuser, non-BYPASSRLS role.
    async fn rls_enforcing_backend() -> PostgresVectorBackend {
        let admin_pool = connect(&dev_database_url()).await.unwrap();
        ensure_extensions(&admin_pool).await.unwrap();
        PostgresVectorBackend::new(admin_pool).await.unwrap();

        let restricted_url =
            dev_database_url().replacen("postgres:postgres@", "z3rno_app:z3rno_app@", 1);
        let pool = connect(&restricted_url).await.unwrap();
        PostgresVectorBackend { pool }
    }

    fn one_hot(index: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBEDDING_DIMS];
        v[index] = 1.0;
        v
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn search_returns_nearest_neighbor() {
        let backend = backend().await;
        let tenant = unique_tenant();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        backend.upsert(&tenant, a, one_hot(0)).await.unwrap();
        backend.upsert(&tenant, b, one_hot(1)).await.unwrap();
        backend.upsert(&tenant, c, one_hot(2)).await.unwrap();

        let mut query = one_hot(0);
        query[1] = 0.1;

        let results = backend.search(&tenant, query, 1).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, a);

        backend.delete(&tenant, a).await.unwrap();
        backend.delete(&tenant, b).await.unwrap();
        backend.delete(&tenant, c).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn search_is_tenant_isolated() {
        let backend = rls_enforcing_backend().await;
        let tenant_a = unique_tenant();
        let tenant_b = unique_tenant();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        backend.upsert(&tenant_a, a, one_hot(0)).await.unwrap();
        backend.upsert(&tenant_b, b, one_hot(0)).await.unwrap();

        let results = backend.search(&tenant_a, one_hot(0), 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, a);

        backend.delete(&tenant_a, a).await.unwrap();
        backend.delete(&tenant_b, b).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn delete_removes_vector_from_search() {
        let backend = backend().await;
        let tenant = unique_tenant();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        backend.upsert(&tenant, a, one_hot(0)).await.unwrap();
        backend.upsert(&tenant, b, one_hot(1)).await.unwrap();

        assert!(backend.delete(&tenant, a).await.unwrap());
        assert!(!backend.delete(&tenant, a).await.unwrap());

        let results = backend.search(&tenant, one_hot(0), 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, b);

        backend.delete(&tenant, b).await.unwrap();
    }
}
