//! Connection + auto-provisioning for the Postgres backend, and the shared
//! tenant-context helper every table-owning implementation (relational,
//! vector) uses to get RLS enforcement.
//!
//! ## Auto-provisioning, not a custom image (decision-doc 0002)
//!
//! `ensure_extensions` runs `CREATE EXTENSION IF NOT EXISTS` against
//! *whatever* Postgres the operator points z3rno at, rather than requiring
//! a bespoke image. Managed Postgres often restricts `CREATE EXTENSION` to
//! a privileged role (RDS/Cloud SQL/etc.), so a failure here is common, not
//! exceptional — the error names exactly which extension failed and points
//! at `bootstrap.sql`, a standalone script a DBA can run by hand once,
//! rather than surfacing an opaque downstream query failure the first time
//! a `vector`/`cypher()` query runs.
//!
//! ## Tenant context: `set_config`, not string-interpolated `SET LOCAL`
//!
//! The old Python system (`z3rno-core/src/z3rno_core/security/rls.py`) set
//! the RLS session variable with `conn.execute(text(f"SET LOCAL
//! app.current_org_id = '{org_id}'"))` — `SET` doesn't accept bind
//! parameters, so that code built the statement via f-string
//! interpolation. In practice `org_id` was always a `UUID` object there, so
//! it likely never carried an attacker-controlled string, but the *shape*
//! is a SQL-injection footgun and z3rno's `tenant_id` is a plain `&str`
//! here, not a `Uuid` — worth actually fixing, not porting faithfully.
//! `SELECT set_config('app.current_tenant_id', $1, true)` is a normal
//! function call, so it takes a real bound parameter; the third argument
//! (`true` = "is_local") gives the exact same transaction-scoped behavior
//! as `SET LOCAL` — it clears on COMMIT/ROLLBACK, so a pooled connection
//! handed to the next unrelated caller starts with no tenant context set.
//!
//! ## Fail-closed by default
//!
//! The RLS policies (see `relational.rs`/`vector.rs`) compare
//! `tenant_id = current_setting('app.current_tenant_id', true)`. The
//! `true` "missing_ok" argument makes `current_setting` return `NULL`
//! instead of raising when the variable was never set, and `tenant_id =
//! NULL` is never true in SQL — so any query that reaches a table without
//! going through `tenant_scoped_tx` first sees zero rows and can insert
//! none, automatically, with no explicit check required in every query.

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};

use crate::backend::error::{BackendError, BackendResult};

/// Standalone SQL a DBA can run by hand once, for environments where the
/// application's runtime role can't `CREATE EXTENSION` itself (common on
/// managed Postgres). Exposed as a constant so a CLI/docs command can print
/// or write it out — see decision-doc 0002 §4.
pub const BOOTSTRAP_SQL: &str = include_str!("bootstrap.sql");

/// Opens a connection pool to `database_url`. Doesn't provision anything —
/// call [`ensure_extensions`] separately so callers can decide whether a
/// provisioning failure should be fatal (it should, for `MemoryEngine`, but
/// a migration tool or health check might want to connect without it).
pub async fn connect(database_url: &str) -> BackendResult<PgPool> {
    let pool = PgPoolOptions::new()
        // Kept deliberately small: each `EngineBackend`/`VectorBackend`/
        // `GraphBackend` opens its own pool against the same
        // `database_url`, so a real deployment (or this crate's own test
        // suite, all three sub-backends' tests running in parallel against
        // one shared dev instance) can have several of these pools alive
        // at once — a high per-pool ceiling multiplies into exhausting the
        // Postgres server's own `max_connections` (100 by default) well
        // before any single pool is actually maxed out (confirmed live:
        // "pool timed out waiting for an open connection" under full test
        // parallelism at the previous ceiling of 10).
        .max_connections(5)
        // sqlx's own default is 30s — fine for a transient pool blip, too
        // slow for "the database is unreachable" to surface at startup, the
        // exact case `MemoryEngine::postgres()` needs to fail fast on.
        .acquire_timeout(std::time::Duration::from_secs(5))
        // search_path is per-connection session state, not database-scoped
        // — a pool hands out whichever physical connection is free, so
        // without this every AGE query would need to defend against
        // landing on a connection that never got `ag_catalog` on its path.
        // Running it once here, as each physical connection is opened,
        // means every connection this pool ever hands out is already
        // AGE-ready, and every backend built on this pool can just query.
        //
        // Deliberately NOT running `LOAD 'age'` here (an earlier version
        // did): `LOAD` requires Postgres *superuser* privilege — confirmed
        // live, `z3rno_app` (a normal, non-superuser app role) got `ERROR:
        // access to library "age" is not allowed`, which surfaced through
        // this hook as every connection attempt silently failing and
        // retrying until the pool's own acquire timeout, i.e. every
        // real non-superuser deployment would have been unable to connect
        // at all. `ensure_extensions` instead sets `session_preload_libraries`
        // once (a privileged, provisioning-time operation), which makes
        // Postgres preload AGE for every session on this database
        // automatically, regardless of which role connects — no per-session
        // `LOAD` needed by anyone.
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET search_path = ag_catalog, \"$user\", public")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(pool)
}

/// Arbitrary but fixed `pg_advisory_lock` key for extension provisioning —
/// distinct from each backend's own schema-init lock key, since this
/// guards a database-wide resource (installed extensions) shared by all of
/// them, not one backend's own table.
const EXTENSIONS_LOCK_KEY: i64 = 0x7a33_726e_6f5f_6578; // "z3rno_ex" in hex-ish

/// Runs `CREATE EXTENSION IF NOT EXISTS` for every extension z3rno's
/// Postgres backend needs (`vector`, `age`). Idempotent, and safe to call
/// concurrently — even `CREATE EXTENSION IF NOT EXISTS` has a race window
/// under true concurrency (confirmed live: `duplicate key value violates
/// unique constraint "pg_extension_name_index"` when multiple callers ran
/// this against a fresh database at once), so the whole sequence runs on
/// one connection held under an advisory lock, same pattern as each
/// backend's own schema-init.
///
/// `LOAD 'age'`/search_path setup is *not* repeated here — `connect`'s
/// `after_connect` hook already puts every connection this pool ever hands
/// out into that state, including the one this function borrows.
pub async fn ensure_extensions(pool: &PgPool) -> BackendResult<()> {
    let mut conn = pool.acquire().await.map_err(anyhow::Error::from)?;

    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(EXTENSIONS_LOCK_KEY)
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

    let mut result = Ok(());
    for ext in ["vector", "age"] {
        result = sqlx::query(&format!("CREATE EXTENSION IF NOT EXISTS {ext}"))
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to provision the Postgres '{ext}' extension: {e}. \
                     The connecting role likely lacks CREATE EXTENSION privilege \
                     (common on managed Postgres). Ask your DBA to run \
                     `z3rno_engine::backend::postgres::BOOTSTRAP_SQL` once with \
                     a privileged role, then retry."
                )
                .into()
            })
            .map(|_| ());
        if result.is_err() {
            break;
        }
    }

    // Makes Postgres preload AGE for every session on this database from
    // here on, regardless of which role connects — the alternative, each
    // session running `LOAD 'age'` itself, requires superuser (confirmed
    // live: a non-superuser role got "access to library \"age\" is not
    // allowed"), which would make auto-provisioning unusable for any real
    // deployment running its application as a non-superuser role, i.e.
    // every deployment that isn't actively a bad idea. `ALTER DATABASE`
    // needs privilege roughly on par with `CREATE EXTENSION` (database
    // owner or superuser), so this fits the same provisioning-time,
    // privileged-role assumption as the extensions loop above, not a new
    // one. Takes effect for connections opened after this point — this
    // pool's own future connections, via `after_connect`'s search_path
    // setup, are exactly that.
    if result.is_ok() {
        let (db_name,): (String,) = sqlx::query_as("SELECT current_database()")
            .fetch_one(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;
        let quoted_db_name = format!("\"{}\"", db_name.replace('"', "\"\""));
        result = sqlx::query(&format!(
            "ALTER DATABASE {quoted_db_name} SET session_preload_libraries = 'age'"
        ))
        .execute(&mut *conn)
        .await
        .map_err(|e| BackendError::from(anyhow::Error::from(e)))
        .map(|_| ());
    }

    // Always unlock, even on failure — see the same reasoning in each
    // backend's schema-init.
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(EXTENSIONS_LOCK_KEY)
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

    result
}

/// Begins a transaction with the RLS tenant context set for `tenant_id`.
/// The context is transaction-local (`set_config(..., true)`) and clears
/// automatically on `commit()`/rollback, so a pooled connection is always
/// handed back with no tenant context — see the module docs for why that's
/// the fail-closed default, not a hole.
pub(crate) async fn tenant_scoped_tx<'a>(
    pool: &PgPool,
    tenant_id: &str,
) -> BackendResult<Transaction<'a, Postgres>> {
    let mut tx = pool.begin().await.map_err(anyhow::Error::from)?;
    sqlx::query("SELECT set_config('app.current_tenant_id', $1, true)")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_database_url() -> String {
        std::env::var("Z3RNO_TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:55433/z3rno_dev".into())
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn ensure_extensions_is_idempotent() {
        let pool = connect(&dev_database_url()).await.unwrap();
        ensure_extensions(&pool).await.unwrap();
        // second call must not error
        ensure_extensions(&pool).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn tenant_context_is_readable_within_its_own_transaction() {
        let pool = connect(&dev_database_url()).await.unwrap();
        let mut tx = tenant_scoped_tx(&pool, "tenant-a").await.unwrap();
        let (value,): (String,) =
            sqlx::query_as("SELECT current_setting('app.current_tenant_id', true)")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(value, "tenant-a");
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn tenant_context_does_not_leak_to_a_fresh_transaction() {
        let pool = connect(&dev_database_url()).await.unwrap();
        {
            let mut tx = tenant_scoped_tx(&pool, "tenant-a").await.unwrap();
            sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
        // a brand new transaction on a (possibly reused, pooled) connection
        // must see no tenant context — proves SET LOCAL-equivalent scoping.
        let mut tx = pool.begin().await.unwrap();
        let (value,): (Option<String>,) =
            sqlx::query_as("SELECT current_setting('app.current_tenant_id', true)")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert!(value.is_none());
    }
}
