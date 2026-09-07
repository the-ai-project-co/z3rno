//! Postgres-backed `EngineBackend`: the `records` table, protected by row-
//! level security instead of SQLite's compound `(tenant_id, id)` primary
//! key. Every query goes through [`super::provision::tenant_scoped_tx`], so
//! Postgres itself enforces tenant isolation even if a query below ever
//! forgot its own `WHERE tenant_id = $N` — and every query below *also*
//! filters explicitly on `tenant_id`, rather than leaning on RLS alone.
//! Belt and suspenders, not redundant: a connection that bypasses RLS
//! (Postgres superusers always do, unconditionally, regardless of `FORCE
//! ROW LEVEL SECURITY`) would otherwise see every tenant's rows through
//! this file's own queries with nothing else to stop it — confirmed live
//! during development, this exact gap let a superuser-connected caller
//! read/search across tenants that RLS alone was supposed to block. See
//! `provision.rs`'s doc comment for why the tenant context is
//! transaction-local and fails closed.
//!
//! ## RLS: single-role, `FORCE ROW LEVEL SECURITY`
//!
//! The old Python system (`012_enable_rls.py`) created two Postgres roles —
//! `z3rno_admin` (bypasses RLS) and `z3rno_app` (subject to it) — and
//! granted table privileges to `z3rno_app` alone. This port deliberately
//! drops that: creating roles needs `CREATE ROLE`, which undermines
//! decision-doc 0002's "works against any stock Postgres, auto-provisioned"
//! goal (managed Postgres commonly restricts role creation the same way it
//! restricts `CREATE EXTENSION` — see `provision.rs`), and there's no
//! admin/bypass use case built yet in this greenfield rewrite to justify
//! the extra role.
//!
//! Instead this uses `ALTER TABLE ... FORCE ROW LEVEL SECURITY`, which
//! makes RLS apply even to the table owner — normally Postgres exempts the
//! owner from its own table's RLS policies, and the auto-provisioning
//! connection that runs `CREATE TABLE` *is* the owner, so without `FORCE`
//! the policy below would silently do nothing for every real connection.
//! This is a considered simplification for a single-role deployment model,
//! not an oversight: revisit if/when z3rno grows an admin-bypass use case.
//!
//! ## Audit-log immutability, adapted to a generic schema
//!
//! The old system had a dedicated `audit_log` table with a trigger
//! blocking all UPDATE/DELETE on it (`014_audit_log_immutable_trigger.py`).
//! z3rno's schema is generic — one `records` table holds both `kind =
//! "memory"` rows (legitimately deleted via `forget`) and `kind =
//! "audit_event"` rows (must never be mutated or deleted once written).
//! The trigger here fires on every UPDATE/DELETE but only raises when
//! `OLD.kind = 'audit_event'`, letting every other kind through untouched.

use async_trait::async_trait;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::super::error::BackendResult;
use super::super::relational::{EngineBackend, Record};
use super::provision::tenant_scoped_tx;

/// Postgres-backed `EngineBackend`. Wraps a pool rather than a single
/// connection — every method borrows a connection for the lifetime of one
/// transaction, matching normal `sqlx` pooled usage.
pub struct PostgresEngineBackend {
    pool: PgPool,
}

/// Arbitrary but fixed `pg_advisory_lock` key, distinct from
/// `vector.rs`'s own schema-init lock key — the two tables' schema-init
/// sequences are unrelated and don't need to serialize against each other,
/// only against concurrent callers of the *same* one.
const SCHEMA_INIT_LOCK_KEY: i64 = 0x7a33_726e_6f5f_7231; // "z3rno_r1" in hex-ish

impl PostgresEngineBackend {
    /// Ensures the `records` table, its RLS policy, and the audit-log
    /// immutability trigger exist, then wraps `pool`. Idempotent — safe to
    /// call on every startup, including concurrently (multiple `z3rno-
    /// server` replicas booting against the same shared Postgres at once):
    /// every statement here runs on one connection held under a Postgres
    /// advisory lock for the whole sequence. Per-statement `IF NOT EXISTS`
    /// isn't enough on its own — `CREATE POLICY`/`CREATE TRIGGER` have no
    /// `IF NOT EXISTS` at all (hence the `DROP ... IF EXISTS` then
    /// `CREATE` pairs below), and even `CREATE TABLE IF NOT EXISTS` has a
    /// known race window under true concurrency (confirmed live: two
    /// callers racing past `new()` at once reliably produced `policy
    /// "tenant_isolation" ... already exists`). The lock makes the whole
    /// sequence atomic with respect to any other caller doing the same
    /// thing, instead of trying to make each statement independently safe.
    pub async fn new(pool: PgPool) -> BackendResult<Self> {
        let mut conn = pool.acquire().await.map_err(anyhow::Error::from)?;

        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(SCHEMA_INIT_LOCK_KEY)
            .execute(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;

        let init_result = Self::init_schema(&mut conn).await;

        // Always unlock, even on failure — an advisory lock left held on a
        // connection that gets handed back to the pool would deadlock the
        // next caller forever.
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(SCHEMA_INIT_LOCK_KEY)
            .execute(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;

        init_result?;
        drop(conn);
        Ok(Self { pool })
    }

    async fn init_schema(conn: &mut sqlx::PgConnection) -> BackendResult<()> {
        // `public.` here is load-bearing, not decoration: every connection
        // this pool hands out has `search_path = ag_catalog, "$user",
        // public` (see `provision.rs`'s `after_connect`), and an
        // unqualified `CREATE TABLE records` always targets the *first*
        // schema in the path regardless of privilege — confirmed live,
        // this table landed in `ag_catalog`, not `public`, before adding
        // the qualifier (the exact bug the slice 0002 spike already
        // documented for the same reason). Every other statement below
        // stays unqualified deliberately — `ALTER TABLE`/`CREATE POLICY`/
        // `CREATE TRIGGER ON records` and this file's queries all resolve
        // an *existing* object by searching the path in order, which
        // correctly finds `public.records` once it's actually there.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS public.records (
                id UUID PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                data JSONB NOT NULL,
                created_at TIMESTAMPTZ NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_records_tenant_kind ON records (tenant_id, kind)",
        )
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

        sqlx::query("ALTER TABLE records ENABLE ROW LEVEL SECURITY")
            .execute(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;
        sqlx::query("ALTER TABLE records FORCE ROW LEVEL SECURITY")
            .execute(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;

        // Policies/triggers have no `IF NOT EXISTS`. An earlier version of
        // this method did `DROP ... IF EXISTS` then `CREATE` unconditionally
        // on every call — which, since `new()` runs on every test/backend
        // construction (not just the first ever), opened a real window on
        // *every single call* where the trigger briefly didn't exist, wide
        // enough for a concurrently-running caller's UPDATE/DELETE on an
        // audit_event row to slip through unrejected (confirmed live: a
        // flaky failure that only reproduced under concurrent test
        // execution, never in isolation). Catching `duplicate_object`
        // instead — try to create, no-op if it's already there — never
        // drops a working policy/trigger just to immediately recreate it,
        // so there's no window at all after the first successful call.
        sqlx::query(
            "DO $$
             BEGIN
                 CREATE POLICY tenant_isolation ON records
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

        sqlx::query(
            "CREATE OR REPLACE FUNCTION records_audit_event_immutable()
             RETURNS TRIGGER AS $$
             BEGIN
                 IF OLD.kind = 'audit_event' THEN
                     RAISE EXCEPTION 'audit_event records are immutable: % is not allowed', TG_OP;
                 END IF;
                 RETURN COALESCE(NEW, OLD);
             END;
             $$ LANGUAGE plpgsql",
        )
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

        sqlx::query(
            "DO $$
             BEGIN
                 CREATE TRIGGER records_no_update_audit_event
                     BEFORE UPDATE ON records
                     FOR EACH ROW
                     EXECUTE FUNCTION records_audit_event_immutable();
             EXCEPTION
                 WHEN duplicate_object THEN NULL;
             END
             $$",
        )
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

        sqlx::query(
            "DO $$
             BEGIN
                 CREATE TRIGGER records_no_delete_audit_event
                     BEFORE DELETE ON records
                     FOR EACH ROW
                     EXECUTE FUNCTION records_audit_event_immutable();
             EXCEPTION
                 WHEN duplicate_object THEN NULL;
             END
             $$",
        )
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

        Ok(())
    }

    fn row_to_record(row: sqlx::postgres::PgRow) -> anyhow::Result<Record> {
        Ok(Record {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            kind: row.try_get("kind")?,
            data: row.try_get("data")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

#[async_trait]
impl EngineBackend for PostgresEngineBackend {
    async fn put(&self, record: Record) -> BackendResult<()> {
        let mut tx = tenant_scoped_tx(&self.pool, &record.tenant_id).await?;
        sqlx::query(
            "INSERT INTO records (id, tenant_id, kind, data, created_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE SET
                tenant_id = excluded.tenant_id,
                kind = excluded.kind,
                data = excluded.data,
                created_at = excluded.created_at",
        )
        .bind(record.id)
        .bind(&record.tenant_id)
        .bind(&record.kind)
        .bind(&record.data)
        .bind(record.created_at)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        Ok(())
    }

    async fn get(&self, tenant_id: &str, id: Uuid) -> BackendResult<Option<Record>> {
        let mut tx = tenant_scoped_tx(&self.pool, tenant_id).await?;
        let row = sqlx::query(
            "SELECT id, tenant_id, kind, data, created_at FROM records \
             WHERE id = $1 AND tenant_id = $2",
        )
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        match row {
            Some(row) => Ok(Some(Self::row_to_record(row)?)),
            None => Ok(None),
        }
    }

    async fn delete(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let mut tx = tenant_scoped_tx(&self.pool, tenant_id).await?;
        let result = sqlx::query("DELETE FROM records WHERE id = $1 AND tenant_id = $2")
            .bind(id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        Ok(result.rows_affected() > 0)
    }

    async fn list(&self, tenant_id: &str, kind: &str) -> BackendResult<Vec<Record>> {
        let mut tx = tenant_scoped_tx(&self.pool, tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, kind, data, created_at FROM records \
             WHERE kind = $1 AND tenant_id = $2",
        )
        .bind(kind)
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        rows.into_iter()
            .map(|row| Self::row_to_record(row).map_err(Into::into))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // NOTE on running this file's tests: each opens its own small pool
    // (`provision::connect`'s `max_connections(5)`) against the shared dev
    // Postgres instance — confirmed stable at full default test
    // parallelism (`--test-threads=8` on an 8-core machine, `cargo test -p
    // z3rno-engine --features testing -- --include-ignored`, run
    // repeatedly against a fresh container) once the real bugs behind the
    // earlier flakiness were actually fixed (see `provision.rs`'s and this
    // file's other doc comments — the `LOAD 'age'` superuser requirement,
    // unqualified `CREATE TABLE` landing in the wrong schema, and the
    // DROP-then-CREATE trigger race). No special invocation is needed.
    //
    // A `static`-shared `PgPool` (via `tokio::sync::OnceCell`) was tried
    // as a fix for what looked like a connection-pressure problem, and
    // reverted: each `#[tokio::test]` spins up its own independent Tokio
    // runtime by default, and a pool's background connection tasks are
    // tied to the runtime that created them — a pool built in test A dies
    // when test A's runtime shuts down, so test B (a different runtime)
    // touching the same shared pool afterward hits "a Tokio 1.x context
    // was found, but it is being shutdown" (confirmed live). Per-test
    // pools are the correct pattern here, not a shared static.

    fn dev_database_url() -> String {
        std::env::var("Z3RNO_TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:55433/z3rno_dev".into())
    }

    async fn test_backend() -> PostgresEngineBackend {
        let pool = super::super::provision::connect(&dev_database_url())
            .await
            .unwrap();
        PostgresEngineBackend::new(pool).await.unwrap()
    }

    /// Postgres never applies row security to a superuser connection —
    /// `FORCE ROW LEVEL SECURITY` only changes owner behavior, and
    /// superusers bypass RLS unconditionally. The shared dev instance's
    /// default credentials (`postgres/postgres`, used by [`test_backend`])
    /// are superuser, so isolation can't actually be observed through it.
    /// `z3rno_app` (provisioned once at container init — see
    /// `postgres/initdb/01-app-role.sql`, not created per-test, which would
    /// itself race under parallel test execution) is a real non-superuser,
    /// non-BYPASSRLS role, so the RLS-dependent tests below exercise the
    /// actual policy instead of skating past it.
    async fn rls_enforcing_test_backend() -> PostgresEngineBackend {
        // Schema/RLS/trigger setup needs owner-or-superuser privilege, so
        // it runs on the superuser pool first.
        let admin_pool = super::super::provision::connect(&dev_database_url())
            .await
            .unwrap();
        PostgresEngineBackend::new(admin_pool).await.unwrap();

        let restricted_url =
            dev_database_url().replacen("postgres:postgres@", "z3rno_app:z3rno_app@", 1);
        let pool = super::super::provision::connect(&restricted_url)
            .await
            .unwrap();
        PostgresEngineBackend { pool }
    }

    fn unique_tenant(label: &str) -> String {
        format!("tenant-{}-{label}", Uuid::new_v4())
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
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn put_then_get_round_trips_through_real_postgres() {
        let backend = test_backend().await;
        let tenant = unique_tenant("roundtrip");
        let record = sample_record(&tenant);

        backend.put(record.clone()).await.unwrap();
        let fetched = backend.get(&tenant, record.id).await.unwrap();

        assert_eq!(fetched, Some(record));
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn get_delete_and_list_are_tenant_scoped_by_rls() {
        let backend = rls_enforcing_test_backend().await;
        let tenant_a = unique_tenant("a");
        let tenant_b = unique_tenant("b");

        let a_memory = sample_record(&tenant_a);
        let mut a_other_kind = sample_record(&tenant_a);
        a_other_kind.kind = "profile".into();
        let b_memory = sample_record(&tenant_b);

        backend.put(a_memory.clone()).await.unwrap();
        backend.put(a_other_kind.clone()).await.unwrap();
        backend.put(b_memory.clone()).await.unwrap();

        // get: tenant B can't see tenant A's row, even by exact id
        assert!(backend.get(&tenant_b, a_memory.id).await.unwrap().is_none());
        assert_eq!(
            backend.get(&tenant_a, a_memory.id).await.unwrap(),
            Some(a_memory.clone())
        );

        // list: each tenant only sees its own "memory"-kind rows, never
        // the other tenant's
        let listed_a = backend.list(&tenant_a, "memory").await.unwrap();
        assert_eq!(listed_a, vec![a_memory.clone()]);
        let listed_b = backend.list(&tenant_b, "memory").await.unwrap();
        assert_eq!(listed_b, vec![b_memory.clone()]);

        // delete: tenant B's delete of tenant A's row affects nothing
        assert!(!backend.delete(&tenant_b, a_memory.id).await.unwrap());
        assert!(backend.get(&tenant_a, a_memory.id).await.unwrap().is_some());

        // tenant A can delete its own row
        assert!(backend.delete(&tenant_a, a_memory.id).await.unwrap());
        assert!(backend.get(&tenant_a, a_memory.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn put_rejects_a_record_whose_tenant_id_does_not_match_the_scoped_tenant() {
        // `EngineBackend::put` takes only a `Record` — no separate "current
        // tenant" argument — so it always scopes its transaction to
        // `record.tenant_id` itself and can never produce a mismatch
        // through the public trait. The scenario this guards against is a
        // caller bug one layer up (transaction opened for tenant A, a
        // record meant for tenant B gets written into it) — exercised
        // here directly against the RLS-enforcing role's pool, the same
        // way `tenant_scoped_tx` + a query are composed inside `put`.
        let backend = rls_enforcing_test_backend().await;
        let scoped_tenant = unique_tenant("scoped");
        let other_tenant = unique_tenant("other");

        let mut tx = tenant_scoped_tx(&backend.pool, &scoped_tenant)
            .await
            .unwrap();
        let result = sqlx::query(
            "INSERT INTO records (id, tenant_id, kind, data, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(&other_tenant)
        .bind("memory")
        .bind(serde_json::json!({"content": "hello"}))
        .bind(Utc::now())
        .execute(&mut *tx)
        .await;

        assert!(
            result.is_err(),
            "RLS WITH CHECK should reject an insert whose tenant_id doesn't match the transaction's tenant context"
        );
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn audit_event_rows_reject_update_and_delete_at_the_db_level() {
        let backend = test_backend().await;
        let tenant = unique_tenant("audit");
        let mut audit_row = sample_record(&tenant);
        audit_row.kind = "audit_event".into();
        backend.put(audit_row.clone()).await.unwrap();

        // Bypass the Rust API entirely: raw SQL, same tenant-scoped tx, to
        // prove the DB-level trigger — not application logic — is what
        // rejects the mutation.
        let mut tx = tenant_scoped_tx(&backend.pool, &tenant).await.unwrap();
        let update_result = sqlx::query("UPDATE records SET data = $1 WHERE id = $2")
            .bind(serde_json::json!({"tampered": true}))
            .bind(audit_row.id)
            .execute(&mut *tx)
            .await;
        assert!(
            update_result.is_err(),
            "trigger should reject UPDATE on an audit_event row"
        );
        tx.rollback().await.unwrap();

        let mut tx = tenant_scoped_tx(&backend.pool, &tenant).await.unwrap();
        let delete_result = sqlx::query("DELETE FROM records WHERE id = $1")
            .bind(audit_row.id)
            .execute(&mut *tx)
            .await;
        assert!(
            delete_result.is_err(),
            "trigger should reject DELETE on an audit_event row"
        );
        tx.rollback().await.unwrap();

        // Row is untouched: still there, unchanged.
        assert_eq!(
            backend.get(&tenant, audit_row.id).await.unwrap(),
            Some(audit_row)
        );
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn non_audit_kinds_can_still_be_updated_and_deleted() {
        let backend = test_backend().await;
        let tenant = unique_tenant("mutable");
        let memory_row = sample_record(&tenant);
        backend.put(memory_row.clone()).await.unwrap();

        // put() is an upsert — updating a "memory" kind row must succeed.
        let mut updated = memory_row.clone();
        updated.data = serde_json::json!({"content": "updated"});
        backend.put(updated.clone()).await.unwrap();
        assert_eq!(
            backend.get(&tenant, memory_row.id).await.unwrap(),
            Some(updated)
        );

        assert!(backend.delete(&tenant, memory_row.id).await.unwrap());
    }
}
