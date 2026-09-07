//! Postgres-backed `GraphBackend`: one Apache AGE graph *per tenant*,
//! selected in `_decision_docs/0008-production-backend-selection.md` and
//! designed against the exact gap that decision doc flagged.
//!
//! ## Isolation: per-tenant AGE graph, not a shared graph + filter
//!
//! Ordinary Postgres RLS (see `relational.rs`/`vector.rs`) covers plain
//! tables, but AGE's graph data lives in per-label tables it creates itself
//! inside a named "graph" — those tables aren't reachable by a `CREATE
//! POLICY` on `records`/`vectors`, so RLS can't cover them. The tempting
//! shortcut — one shared AGE graph, every node/edge carrying a `tenant_id`
//! property, every `cypher()` call filtered by it — is *exactly* the
//! isolation model that got the Neo4j+Qdrant candidate rejected in slice
//! 0002: it's filter-discipline-only, so a query that simply omits the
//! filter (a bug, not an attack) leaks every tenant's data.
//!
//! Instead, [`graph_name_for_tenant`] derives a distinct AGE graph name per
//! `tenant_id`, and every method here creates/targets *that* graph and no
//! other. AGE's `create_graph(name)` provisions its own Postgres schema —
//! see the `tenant_isolation_is_structural_not_filter_based` test below,
//! which inspects `ag_catalog.ag_graph` directly and asserts two tenants
//! produce two distinct graphs/schemas, not one shared graph with rows
//! scoped by a property. There is no code path here that lets a caller
//! choose which tenant's graph a query hits — the graph name is always
//! computed from the `tenant_id` the trait method itself received, never
//! taken from Cypher text or any other caller-controlled string.
//!
//! ## Known rough edge: hand-built `cypher()` strings
//!
//! Rust has no mature AGE ORM/query builder (unlike Python's SQLAlchemy AGE
//! extensions), so — per the plan doc (0004.3) — this builds `cypher()` SQL
//! text by hand via `sqlx`'s raw query support, closer to the old Python
//! system's approach than the embedded (`petgraph`) graph store is. That's
//! a deliberate, accepted rough edge for this sub-slice, not hidden: every
//! query is assembled with [`format!`] below rather than a query builder.
//!
//! ## Parameterization: what AGE actually lets you bind, and what it doesn't
//!
//! AGE's `cypher()` signature is `cypher(graph_name, query_text, params)`.
//! Probing the live dev instance directly (`psql` against
//! `postgres://postgres:postgres@localhost:55433/z3rno_dev`) turned up two
//! hard constraints that shape everything below:
//!
//! - **`graph_name` must be a literal constant**, not a bind parameter —
//!   `cypher($1, $$ ... $$)` fails with `a name constant is expected` even
//!   inside a `PREPARE`. It has to be formatted directly into the SQL text.
//!   This is fine here because it's *never* caller-controlled text — it's
//!   always [`graph_name_for_tenant`]'s output, a `z3rno_t_<32 hex chars>`
//!   string, so there's nothing to inject.
//! - **The `params` argument must be an actual bind parameter typed as
//!   `agtype`**, not a literal or a cast expression — `cypher(g, $$ ...
//!   $$, $1::agtype)` fails with `third argument of cypher function must
//!   be a parameter` even though `$1::agtype` *is* a parameter, just a cast
//!   one. Only a parameter whose declared type is already `agtype` (as in
//!   `PREPARE p(agtype) AS ...`) is accepted, and `sqlx` doesn't know how
//!   to bind Rust values as the `agtype` OID without a hand-written
//!   `sqlx::Type`/`Encode` impl — more machinery than this sub-slice's
//!   scope justifies for the one or two values each query needs.
//!
//! So values that must appear *inside* the Cypher body (`$$ ... $$`) are
//! interpolated via [`cypher_literal`], not bound: every node id is a
//! `Uuid`, whose canonical hyphenated form ([`uuid::Uuid`]'s `Display`) can
//! only ever contain `[0-9a-f-]` and therefore needs no escaping at all;
//! every relationship name is caller-supplied text, so it goes through
//! [`cypher_literal`], which backslash-escapes Cypher's own string-literal
//! metacharacters (`\` and `'`) and, separately, rejects the literal
//! sequence `$$` outright — that sequence isn't a Cypher concern at all,
//! it's the *outer* SQL dollar-quote delimiter, so escaping can't neutralize
//! it and the only safe move is refusing the value. Everything that isn't
//! interpolated into the Cypher body — the graph-existence check, tenant
//! isolation test assertions — uses ordinary bound `$1` parameters the
//! normal `sqlx` way.
//!
//! ## One fixed edge label, relationship name as a property
//!
//! AGE's Cypher subset doesn't support a dynamic/parameterized edge label
//! (`-[:$var]->`), so every edge is created with the single fixed label
//! `RELATES` and the caller's relationship name stored as a `name`
//! property instead (`-[:RELATES {name: 'follows'}]->`). `neighbors`
//! filters on that property (`WHERE r.name = '...'`) rather than on the
//! edge label. `GraphEdge.relationship` is still exactly what callers see
//! through the trait — this is purely an AGE storage-layer choice.
//!
//! ## `agtype` results: only ever return scalars
//!
//! AGE's `agtype` has no `sqlx::Type` impl, so results are read back by
//! casting the `cypher()` output column to `::text` in the outer SQL and
//! parsing that string in Rust. That cast works cleanly for scalar agtype
//! values (numbers, strings) but **fails outright for composite values**
//! (`agtype_value_to_text: unsupported argument agtype 6` — confirmed live
//! against vertex and edge return values). The fix isn't parsing around
//! that error, it's never triggering it: every Cypher query below `RETURN`s
//! only scalar properties (`n.id`, `b.id`, `count(n)`), never a bare node
//! or edge variable. A returned id comes back as `"<uuid-text>"` (quoted,
//! since agtype's text form is JSON-ish) and is stripped of its surrounding
//! quotes before `Uuid::parse_str`.
//!
//! ## Session setup: `LOAD`/`search_path` is per-connection, handled by the pool
//!
//! `LOAD 'age'`/`search_path` are *session-scoped*, not database-scoped —
//! a naive one-time provisioning step could leave later queries landing on
//! a different physical connection than the one it set up (confirmed live
//! during development: `ag_catalog.cypher()` fails with `operator class
//! "graphid_ops" does not exist for access method "btree"` on a connection
//! that loaded AGE but never got `search_path` set). `provision::connect`
//! fixes this at the source via a `PgPoolOptions::after_connect` hook that
//! runs on every physical connection as it's opened, so every connection
//! this pool ever hands out is already AGE-ready — the methods below just
//! acquire and query, no per-call reissue.

use uuid::Uuid;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Row};

use super::super::error::BackendResult;
use super::super::graph::{GraphBackend, GraphEdge};

/// Derives a bounded-length, valid AGE graph name deterministically from an
/// arbitrary `tenant_id`. `tenant_id` can contain characters that aren't
/// valid in a Postgres identifier and can be arbitrarily long, so it's
/// never used directly — this hashes it and keeps only the first 16 bytes
/// (32 hex chars) of the digest, giving a 40-char `z3rno_t_<hex>` name
/// (Postgres's identifier limit is 63 bytes) that's the same every time for
/// the same `tenant_id` and collision-astronomically-unlikely across
/// different ones.
fn graph_name_for_tenant(tenant_id: &str) -> String {
    let digest = Sha256::digest(tenant_id.as_bytes());
    format!("z3rno_t_{:x}", digest)[..40].to_string()
}

/// Escapes `s` for interpolation as a single-quoted Cypher string literal
/// embedded inside a `$$ ... $$`-delimited SQL literal. See the module docs
/// for why this can't be a bound parameter. Backslash and single-quote are
/// Cypher string-literal metacharacters, escaped the normal way; the
/// literal sequence `$$` is the *outer* SQL delimiter, not something Cypher
/// escaping can neutralize, so a value containing it is rejected outright
/// rather than mangled.
fn cypher_literal(s: &str) -> BackendResult<String> {
    if s.contains("$$") {
        return Err(anyhow::anyhow!(
            "value contains '$$', which cannot appear in a dollar-quoted \
             Cypher literal: {s:?}"
        )
        .into());
    }
    Ok(s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Strips the surrounding `"..."` that `agtype`'s text form wraps string
/// scalars in (its text output is JSON-ish), then parses the remainder as
/// a `Uuid`. Node ids only ever come back this way — see module docs on
/// why every query `RETURN`s a scalar property rather than a whole node.
fn parse_agtype_uuid(text: &str) -> BackendResult<Uuid> {
    let inner = text.trim_matches('"');
    Uuid::parse_str(inner)
        .map_err(|e| anyhow::anyhow!("unexpected agtype id value {text:?}: {e}").into())
}

/// Acquires one pooled connection, already AGE-ready. `provision::connect`
/// runs `LOAD 'age'`/sets the search path via `after_connect` on every
/// physical connection as it's opened, so every connection this pool hands
/// out is ready for `ag_catalog` — no per-call reissue needed here. See
/// module docs for why that matters (this is per-connection session state,
/// not database-scoped).
async fn acquire_age_ready(
    pool: &PgPool,
) -> BackendResult<sqlx::pool::PoolConnection<sqlx::Postgres>> {
    pool.acquire()
        .await
        .map_err(|e| anyhow::Error::from(e).into())
}

/// Creates `graph_name`'s AGE graph if it doesn't already exist. Checks
/// `ag_catalog.ag_graph` first (a normal bound-parameter query) to avoid
/// the round trip through an error in the common case, then falls back to
/// calling `create_graph` and swallowing only the specific "already
/// exists" failure (SQLSTATE `3F000`, confirmed live) — a concurrent
/// creator racing this check is the only way that fires, and losing that
/// race is success, not an error. Any other failure propagates.
async fn ensure_tenant_graph(conn: &mut PgConnection, graph_name: &str) -> BackendResult<()> {
    let exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM ag_catalog.ag_graph WHERE name = $1")
            .bind(graph_name)
            .fetch_optional(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;
    if exists.is_some() {
        return Ok(());
    }
    let result = sqlx::query("SELECT create_graph($1)")
        .bind(graph_name)
        .execute(&mut *conn)
        .await;
    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("3F000") => Ok(()),
        Err(e) => Err(anyhow::Error::from(e).into()),
    }
}

/// `GraphBackend` implementation over Apache AGE, one graph per tenant. See
/// module docs for the isolation design, the `cypher()` parameterization
/// approach, and the `agtype` result-parsing rules this file follows.
pub struct PostgresGraphBackend {
    pool: PgPool,
}

impl PostgresGraphBackend {
    /// Wraps `pool`. Doesn't provision anything itself — extensions are
    /// `provision::ensure_extensions`'s job; per-tenant graphs are created
    /// lazily on first use by each method below.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl GraphBackend for PostgresGraphBackend {
    async fn add_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<()> {
        let graph = graph_name_for_tenant(tenant_id);
        let mut conn = acquire_age_ready(&self.pool).await?;
        ensure_tenant_graph(&mut conn, &graph).await?;

        let sql = format!(
            "SELECT * FROM cypher('{graph}', $$ MERGE (:Memory {{id: '{id}'}}) $$) AS (v agtype)"
        );
        sqlx::query(&sql)
            .execute(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(())
    }

    async fn add_edge(&self, tenant_id: &str, edge: GraphEdge) -> BackendResult<()> {
        let graph = graph_name_for_tenant(tenant_id);
        let mut conn = acquire_age_ready(&self.pool).await?;
        ensure_tenant_graph(&mut conn, &graph).await?;

        let relationship = cypher_literal(&edge.relationship)?;
        let source = edge.source;
        let target = edge.target;
        // MERGE the endpoints first (matching the embedded backend's
        // add_edge: it auto-creates missing nodes rather than erroring),
        // then MERGE the edge itself so add_edge is idempotent too.
        let sql = format!(
            "SELECT * FROM cypher('{graph}', $$ \
                MERGE (a:Memory {{id: '{source}'}}) \
                MERGE (b:Memory {{id: '{target}'}}) \
                MERGE (a)-[:RELATES {{name: '{relationship}'}}]->(b) \
             $$) AS (v agtype)"
        );
        sqlx::query(&sql)
            .execute(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(())
    }

    async fn neighbors(
        &self,
        tenant_id: &str,
        id: Uuid,
        relationship: Option<&str>,
    ) -> BackendResult<Vec<Uuid>> {
        let graph = graph_name_for_tenant(tenant_id);
        let mut conn = acquire_age_ready(&self.pool).await?;
        ensure_tenant_graph(&mut conn, &graph).await?;

        let where_clause = match relationship {
            Some(r) => format!("WHERE r.name = '{}'", cypher_literal(r)?),
            None => String::new(),
        };
        let sql = format!(
            "SELECT b_id::text FROM cypher('{graph}', $$ \
                MATCH (a:Memory {{id: '{id}'}})-[r:RELATES]->(b:Memory) \
                {where_clause} \
                RETURN b.id \
             $$) AS (b_id agtype)"
        );
        let rows = sqlx::query(&sql)
            .fetch_all(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;
        rows.into_iter()
            .map(|row| parse_agtype_uuid(row.get::<String, _>(0).as_str()))
            .collect()
    }

    async fn remove_node(&self, tenant_id: &str, id: Uuid) -> BackendResult<bool> {
        let graph = graph_name_for_tenant(tenant_id);
        let mut conn = acquire_age_ready(&self.pool).await?;
        ensure_tenant_graph(&mut conn, &graph).await?;

        // DETACH DELETE removes the node and every incident edge in one
        // statement; returning n.id before deletion (Cypher evaluates
        // RETURN against the bound variable, not the post-delete state)
        // doubles as the existence check `remove_node`'s `bool` needs — no
        // rows means there was nothing to delete.
        let sql = format!(
            "SELECT n_id::text FROM cypher('{graph}', $$ \
                MATCH (n:Memory {{id: '{id}'}}) \
                DETACH DELETE n \
                RETURN n.id \
             $$) AS (n_id agtype)"
        );
        let rows = sqlx::query(&sql)
            .fetch_all(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(!rows.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn dev_database_url() -> String {
        std::env::var("Z3RNO_TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:55433/z3rno_dev".into())
    }

    async fn backend() -> PostgresGraphBackend {
        let pool = super::super::provision::connect(&dev_database_url())
            .await
            .unwrap();
        super::super::provision::ensure_extensions(&pool)
            .await
            .unwrap();
        PostgresGraphBackend::new(pool)
    }

    fn unique_tenant(suffix: &str) -> String {
        format!("tenant-{}-{suffix}", Uuid::new_v4())
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn add_and_traverse_edge() {
        let backend = backend().await;
        let tenant = unique_tenant("a");
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        backend.add_node(&tenant, a).await.unwrap();
        backend.add_node(&tenant, b).await.unwrap();
        backend
            .add_edge(
                &tenant,
                GraphEdge {
                    source: a,
                    target: b,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();

        let neighbors = backend.neighbors(&tenant, a, None).await.unwrap();
        assert_eq!(neighbors, vec![b]);

        let filtered = backend.neighbors(&tenant, a, Some("other")).await.unwrap();
        assert!(filtered.is_empty());

        let matched = backend
            .neighbors(&tenant, a, Some("relates_to"))
            .await
            .unwrap();
        assert_eq!(matched, vec![b]);
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn remove_node_cleans_up_incident_edges() {
        let backend = backend().await;
        let tenant = unique_tenant("a");
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        backend.add_node(&tenant, a).await.unwrap();
        backend.add_node(&tenant, b).await.unwrap();
        backend.add_node(&tenant, c).await.unwrap();
        backend
            .add_edge(
                &tenant,
                GraphEdge {
                    source: a,
                    target: b,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();
        backend
            .add_edge(
                &tenant,
                GraphEdge {
                    source: c,
                    target: a,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();

        assert!(backend.remove_node(&tenant, a).await.unwrap());
        // second removal is a no-op, not an error
        assert!(!backend.remove_node(&tenant, a).await.unwrap());

        assert!(backend
            .neighbors(&tenant, a, None)
            .await
            .unwrap()
            .is_empty());
        assert!(backend
            .neighbors(&tenant, c, None)
            .await
            .unwrap()
            .is_empty());

        // b and c are still independently usable as graph nodes.
        backend
            .add_edge(
                &tenant,
                GraphEdge {
                    source: b,
                    target: c,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(backend.neighbors(&tenant, b, None).await.unwrap(), vec![c]);
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn add_edge_auto_creates_missing_nodes() {
        let backend = backend().await;
        let tenant = unique_tenant("a");
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        // Neither node was registered via add_node first.
        backend
            .add_edge(
                &tenant,
                GraphEdge {
                    source: a,
                    target: b,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();

        assert_eq!(backend.neighbors(&tenant, a, None).await.unwrap(), vec![b]);
    }

    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn tenants_are_isolated_by_query_results() {
        let backend = backend().await;
        let tenant_a = unique_tenant("a");
        let tenant_b = unique_tenant("b");
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        backend.add_node(&tenant_a, a).await.unwrap();
        backend.add_node(&tenant_a, b).await.unwrap();
        backend
            .add_edge(
                &tenant_a,
                GraphEdge {
                    source: a,
                    target: b,
                    relationship: "relates_to".into(),
                },
            )
            .await
            .unwrap();

        // tenant_b never saw these nodes/edges at all.
        assert!(backend
            .neighbors(&tenant_b, a, None)
            .await
            .unwrap()
            .is_empty());
        assert!(!backend.remove_node(&tenant_b, a).await.unwrap());
    }

    /// The acceptance bar this sub-slice actually cares about: not just
    /// "the API returns the right thing" but "there genuinely are two
    /// separate AGE graphs (two separate Postgres schemas) on disk", by
    /// inspecting `ag_catalog.ag_graph` directly rather than trusting
    /// query results alone.
    #[tokio::test]
    #[ignore = "requires a running Postgres — see backend/postgres/docker-compose.dev.yml"]
    async fn tenant_isolation_is_structural_not_filter_based() {
        let backend = backend().await;
        let tenant_a = unique_tenant("a");
        let tenant_b = unique_tenant("b");

        backend.add_node(&tenant_a, Uuid::new_v4()).await.unwrap();
        backend.add_node(&tenant_b, Uuid::new_v4()).await.unwrap();

        let graph_a = graph_name_for_tenant(&tenant_a);
        let graph_b = graph_name_for_tenant(&tenant_b);
        assert_ne!(
            graph_a, graph_b,
            "distinct tenants must derive distinct graph names"
        );

        let mut conn = acquire_age_ready(&backend.pool).await.unwrap();
        let names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM ag_catalog.ag_graph WHERE name = ANY($1)")
                .bind(vec![graph_a.clone(), graph_b.clone()])
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        // Two distinct rows in ag_catalog.ag_graph — i.e. two genuinely
        // separate AGE graphs — not one shared graph both tenants wrote
        // into under a tenant_id filter.
        assert_eq!(
            names.len(),
            2,
            "expected two separate underlying AGE graphs, found {names:?}"
        );
        assert!(names.contains(&graph_a));
        assert!(names.contains(&graph_b));

        // And each graph name is also a real, distinct Postgres schema —
        // AGE's per-tenant "graph" is backed by its own schema/tables, not
        // a shared one.
        let schemas: Vec<String> = sqlx::query_scalar(
            "SELECT schema_name FROM information_schema.schemata WHERE schema_name = ANY($1)",
        )
        .bind(vec![graph_a.clone(), graph_b.clone()])
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            schemas.len(),
            2,
            "expected two separate Postgres schemas, found {schemas:?}"
        );
    }

    #[test]
    fn graph_name_is_deterministic_and_bounded() {
        let a = graph_name_for_tenant("tenant-abc");
        let b = graph_name_for_tenant("tenant-abc");
        let c = graph_name_for_tenant("tenant-xyz");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.len() <= 63, "must fit Postgres's identifier length limit");
        assert!(a.starts_with("z3rno_t_"));
    }

    #[test]
    fn cypher_literal_rejects_dollar_dollar() {
        assert!(cypher_literal("has $$ in it").is_err());
    }

    #[test]
    fn cypher_literal_escapes_quotes_and_backslashes() {
        assert_eq!(cypher_literal("o'brien").unwrap(), "o\\'brien");
        assert_eq!(cypher_literal("back\\slash").unwrap(), "back\\\\slash");
    }
}
