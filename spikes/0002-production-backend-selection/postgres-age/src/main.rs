//! Structural spike: Postgres + pgvector + Apache AGE, driven from Rust via sqlx.
//!
//! Not a quality spike (embeddings are random floats), a structural one: prove each
//! rubric-relevant capability actually works against a real running Postgres.
//!
//! Run `docker compose up -d --build` in this directory first, then `cargo run`.

use rand::Rng;
use sqlx::{postgres::PgConnectOptions, ConnectOptions, Connection, PgConnection, Row};
use std::str::FromStr;

const HOST: &str = "127.0.0.1";
const PORT: u16 = 55432;
const DB: &str = "z3rno_spike";
const GRAPH: &str = "spike_graph";

fn connect_opts(user: &str, password: &str) -> PgConnectOptions {
    PgConnectOptions::from_str(&format!("postgres://{user}:{password}@{HOST}:{PORT}/{DB}"))
        .expect("valid connect url")
        .disable_statement_logging()
}

async fn connect(user: &str, password: &str) -> sqlx::Result<PgConnection> {
    PgConnection::connect_with(&connect_opts(user, password)).await
}

/// AGE needs `LOAD 'age'` + a search_path containing ag_catalog on *every* session
/// before `cypher(...)` is usable.
async fn load_age(conn: &mut PgConnection) -> sqlx::Result<()> {
    sqlx::query("LOAD 'age'").execute(&mut *conn).await?;
    sqlx::query("SET search_path = ag_catalog, \"$user\", public")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

fn random_embedding(dim: usize) -> Vec<f32> {
    let mut rng = rand::thread_rng();
    (0..dim).map(|_| rng.gen_range(-1.0f32..1.0f32)).collect()
}

fn vector_literal(v: &[f32]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn pass(step: &str, detail: &str) {
    println!("PASS  {step}: {detail}");
}

fn fail(step: &str, detail: &str) {
    println!("FAIL  {step}: {detail}");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("== postgres-age spike ==\n");

    // --- 0. Admin setup: extensions, schema, RLS policy, tenant roles -----------------
    let mut admin = connect("postgres", "postgres").await?;

    match sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(&mut admin)
        .await
    {
        Ok(_) => pass("extensions", "CREATE EXTENSION vector ok"),
        Err(e) => {
            fail("extensions", &format!("vector: {e}"));
            return Ok(());
        }
    }
    match sqlx::query("CREATE EXTENSION IF NOT EXISTS age")
        .execute(&mut admin)
        .await
    {
        Ok(_) => pass("extensions", "CREATE EXTENSION age ok"),
        Err(e) => {
            fail("extensions", &format!("age: {e}"));
            return Ok(());
        }
    }
    // Table setup must happen on the default (public-first) search_path — load_age()
    // below prepends ag_catalog, and unqualified CREATE TABLE picks the *first* schema
    // in search_path, not just any schema the role can write to.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS memories (
            id uuid primary key default gen_random_uuid(),
            tenant_id text not null,
            content text not null,
            embedding vector(384) not null
        )",
    )
    .execute(&mut admin)
    .await?;
    sqlx::query("TRUNCATE memories").execute(&mut admin).await?;
    sqlx::query("ALTER TABLE memories ENABLE ROW LEVEL SECURITY")
        .execute(&mut admin)
        .await?;
    sqlx::query("DROP POLICY IF EXISTS tenant_isolation ON memories")
        .execute(&mut admin)
        .await?;
    // Isolation tied to the connecting Postgres role's identity, not a session var an
    // application could forget to set.
    sqlx::query("CREATE POLICY tenant_isolation ON memories USING (tenant_id = current_user)")
        .execute(&mut admin)
        .await?;

    for role in ["tenant_a", "tenant_b"] {
        // DROP OWNED first: re-running this spike would otherwise fail to drop a role
        // that already holds GRANTs from a prior run.
        sqlx::query(&format!("DROP OWNED BY {role}"))
            .execute(&mut admin)
            .await
            .ok();
        sqlx::query(&format!("DROP ROLE IF EXISTS {role}"))
            .execute(&mut admin)
            .await?;
        sqlx::query(&format!("CREATE ROLE {role} LOGIN PASSWORD '{role}_pw'"))
            .execute(&mut admin)
            .await?;
        sqlx::query(&format!("GRANT SELECT, INSERT ON memories TO {role}"))
            .execute(&mut admin)
            .await?;
        sqlx::query(&format!("GRANT USAGE ON SCHEMA public TO {role}"))
            .execute(&mut admin)
            .await?;
    }
    pass(
        "rls-setup",
        "table + policy + tenant_a/tenant_b roles created",
    );

    load_age(&mut admin).await?;
    match sqlx::query(&format!(
        "SELECT * FROM ag_catalog.ag_graph WHERE name = '{GRAPH}'"
    ))
    .fetch_optional(&mut admin)
    .await?
    {
        Some(_) => {}
        None => {
            sqlx::query(&format!("SELECT create_graph('{GRAPH}')"))
                .execute(&mut admin)
                .await?;
        }
    }
    pass("graph-setup", &format!("graph '{GRAPH}' ready"));

    // --- 1. Store a memory (as tenant_a) + recall via vector similarity ---------------
    let mut tenant_a = connect("tenant_a", "tenant_a_pw").await?;

    let stored_embedding = random_embedding(384);
    let content = "The user prefers dark mode and dislikes push notifications.";
    let row = sqlx::query(
        "INSERT INTO memories (tenant_id, content, embedding) VALUES ($1, $2, $3::vector) RETURNING id",
    )
    .bind("tenant_a")
    .bind(content)
    .bind(vector_literal(&stored_embedding))
    .fetch_one(&mut tenant_a)
    .await?;
    let stored_id: uuid::Uuid = row.get("id");
    pass("store-memory", &format!("id={stored_id} tenant=tenant_a"));

    // A query embedding close to the stored one (small jitter) so similarity search has
    // a real nearest neighbor to find, not just a coincidence with dim=1 row.
    let mut rng = rand::thread_rng();
    let query_embedding: Vec<f32> = stored_embedding
        .iter()
        .map(|f| f + rng.gen_range(-0.01f32..0.01f32))
        .collect();

    let row = sqlx::query(
        "SELECT id, content, embedding <=> $1::vector AS distance
         FROM memories
         ORDER BY embedding <=> $1::vector
         LIMIT 1",
    )
    .bind(vector_literal(&query_embedding))
    .fetch_one(&mut tenant_a)
    .await?;
    let recalled_id: uuid::Uuid = row.get("id");
    let distance: f64 = row.get("distance");
    if recalled_id == stored_id {
        pass(
            "vector-recall",
            &format!("<=> cosine distance {distance:.6}, matched stored row"),
        );
    } else {
        fail("vector-recall", "nearest neighbor did not match stored row");
    }

    // --- 2. Graph node + edge via Cypher-via-SQL, then traverse -----------------------
    load_age(&mut admin).await?;
    sqlx::query(&format!(
        "SELECT * FROM cypher('{GRAPH}', $$
            CREATE (a:Memory {{name: 'root memory', tenant_id: 'tenant_a'}})
        $$) AS (v agtype)"
    ))
    .execute(&mut admin)
    .await?;
    sqlx::query(&format!(
        "SELECT * FROM cypher('{GRAPH}', $$
            MATCH (a:Memory {{name: 'root memory'}})
            CREATE (b:Memory {{name: 'related memory', tenant_id: 'tenant_a'}})
            CREATE (a)-[r:RELATES_TO]->(b)
        $$) AS (v agtype)"
    ))
    .execute(&mut admin)
    .await?;
    pass("graph-write", "created 2 nodes + 1 RELATES_TO edge");

    let row = sqlx::query(&format!(
        "SELECT a_name::text AS a_name, r_label::text AS r_label, b_name::text AS b_name
         FROM cypher('{GRAPH}', $$
            MATCH (a:Memory {{name: 'root memory'}})-[r:RELATES_TO]->(b:Memory)
            RETURN a.name, label(r), b.name
         $$) AS (a_name agtype, r_label agtype, b_name agtype)"
    ))
    .fetch_one(&mut admin)
    .await?;
    let a_name: String = row.get("a_name");
    let r_label: String = row.get("r_label");
    let b_name: String = row.get("b_name");
    pass(
        "graph-traverse",
        &format!("{a_name} -[{r_label}]-> {b_name}"),
    );

    // --- 3. Multi-tenant isolation: tenant_b must not see or write tenant_a's row -----
    let mut tenant_b = connect("tenant_b", "tenant_b_pw").await?;

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM memories")
        .fetch_one(&mut tenant_b)
        .await?;
    if count == 0 {
        pass(
            "rls-read-isolation",
            "tenant_b SELECT sees 0 rows (tenant_a's row hidden)",
        );
    } else {
        fail(
            "rls-read-isolation",
            &format!("tenant_b saw {count} row(s) — leak!"),
        );
    }

    let forged_insert = sqlx::query(
        "INSERT INTO memories (tenant_id, content, embedding) VALUES ('tenant_a', 'forged', $1::vector)",
    )
    .bind(vector_literal(&random_embedding(384)))
    .execute(&mut tenant_b)
    .await;
    match forged_insert {
        Err(e) if e.to_string().contains("row-level security") => {
            pass(
                "rls-write-isolation",
                "tenant_b INSERT tagged tenant_id='tenant_a' rejected by RLS policy",
            );
        }
        Err(e) => fail("rls-write-isolation", &format!("unexpected error: {e}")),
        Ok(_) => fail(
            "rls-write-isolation",
            "tenant_b was able to write tenant_a's data!",
        ),
    }

    admin.close().await.ok();
    tenant_a.close().await.ok();
    tenant_b.close().await.ok();

    println!("\n== done ==");
    Ok(())
}
