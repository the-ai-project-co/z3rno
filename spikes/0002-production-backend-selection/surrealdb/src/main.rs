//! Standalone spike for candidate "SurrealDB" (see ../RUBRIC.md).
//!
//! Not shipped product code. Connects to a real `surrealdb/surrealdb` docker
//! container (see docker-compose.yml) and drives it end to end:
//!   1. store a "memory" record (text + fake 384-dim embedding + tenant)
//!   2. recall it via native vector search
//!   3. RELATE two records and traverse the graph edge back out
//!   4. prove tenant isolation via SurrealDB's native record-access + row
//!      permissions (not hand-rolled app-level filtering)
//!
//! Every step prints PASS/FAIL so the result is legible without re-reading
//! the code.

use rand::Rng;
use surrealdb::engine::remote::ws::Ws;
use surrealdb::opt::auth::{Record as RecordAccess, Root};
use surrealdb::types::{RecordId, SurrealValue};
use surrealdb::Surreal;

const NS: &str = "z3rno_spike";
const DB: &str = "spike";
const EMBED_DIM: usize = 384;

#[derive(Debug, Clone, SurrealValue)]
struct MemoryRecord {
    id: RecordId,
    text: String,
    tenant: String,
}

#[derive(Debug, SurrealValue)]
struct RelatedText {
    related: Vec<String>,
}

#[derive(Debug, SurrealValue)]
struct TenantCreds {
    tenant: String,
    pass: String,
}

fn random_embedding(dim: usize) -> Vec<f32> {
    let mut rng = rand::thread_rng();
    (0..dim).map(|_| rng.gen_range(-1.0f32..1.0f32)).collect()
}

fn report(step: &str, ok: bool, detail: impl AsRef<str>) {
    let mark = if ok { "PASS" } else { "FAIL" };
    println!("[{mark}] {step}: {}", detail.as_ref());
}

#[tokio::main]
async fn main() -> surrealdb::Result<()> {
    println!("== SurrealDB spike ==");

    // --- connect as root and lay down schema -------------------------------
    let root = Surreal::new::<Ws>("127.0.0.1:8000").await?;
    root.signin(Root {
        username: "root".to_string(),
        password: "root".to_string(),
    })
    .await?;
    root.use_ns(NS).use_db(DB).await?;

    // OVERWRITE (not IF NOT EXISTS) so re-running this spike against the same
    // persistent rocksdb volume redefines cleanly instead of erroring.
    let schema = format!(
        r#"
        REMOVE TABLE IF EXISTS memory;
        REMOVE TABLE IF EXISTS relates_to;
        REMOVE TABLE IF EXISTS user;
        REMOVE ACCESS IF EXISTS tenant_access ON DATABASE;
        DEFINE TABLE user SCHEMALESS PERMISSIONS NONE;
        DEFINE TABLE memory SCHEMALESS
            PERMISSIONS
                FOR select, update, delete WHERE tenant = $auth.tenant
                FOR create WHERE tenant = $auth.tenant;
        DEFINE TABLE relates_to SCHEMALESS
            PERMISSIONS
                FOR select WHERE in.tenant = $auth.tenant
                FOR create WHERE in.tenant = $auth.tenant AND out.tenant = $auth.tenant;
        DEFINE INDEX memory_embedding_idx ON memory FIELDS embedding HNSW DIMENSION {EMBED_DIM} DIST COSINE;
        DEFINE ACCESS tenant_access ON DATABASE TYPE RECORD
            SIGNUP ( CREATE user SET tenant = $tenant, pass = crypto::argon2::generate($pass) )
            SIGNIN ( SELECT * FROM user WHERE tenant = $tenant AND crypto::argon2::compare(pass, $pass) )
            DURATION FOR TOKEN 1h, FOR SESSION 12h;
        "#
    );
    match root.query(schema).await?.check() {
        Ok(_) => report(
            "schema setup",
            true,
            "user/memory/relates_to tables, HNSW index, tenant_access defined",
        ),
        Err(e) => {
            report("schema setup", false, e.to_string());
            return Ok(());
        }
    }

    // --- two tenants, each an independent authenticated session ------------
    // `root.clone()` shares the one websocket connection but gets its own
    // session (auth/ns/db context) - see surrealdb's own session_isolation
    // integration tests for this exact pattern.
    let acme = root.clone();
    let acme_signup = acme
        .signup(RecordAccess {
            namespace: NS.to_string(),
            database: DB.to_string(),
            access: "tenant_access".to_string(),
            params: TenantCreds {
                tenant: "acme".to_string(),
                pass: "acme-secret".to_string(),
            },
        })
        .await;
    report(
        "tenant signup (acme)",
        acme_signup.is_ok(),
        format!("{acme_signup:?}")
            .chars()
            .take(80)
            .collect::<String>(),
    );

    let globex = root.clone();
    let globex_signup = globex
        .signup(RecordAccess {
            namespace: NS.to_string(),
            database: DB.to_string(),
            access: "tenant_access".to_string(),
            params: TenantCreds {
                tenant: "globex".to_string(),
                pass: "globex-secret".to_string(),
            },
        })
        .await;
    report(
        "tenant signup (globex)",
        globex_signup.is_ok(),
        format!("{globex_signup:?}")
            .chars()
            .take(80)
            .collect::<String>(),
    );

    // --- 1. store one memory record (text + fake embedding + tenant) -------
    let query_embedding = random_embedding(EMBED_DIM);
    let mem_a: surrealdb::Result<Vec<MemoryRecord>> = (|| async {
        acme.query("CREATE memory SET text = $text, embedding = $embedding, tenant = $tenant")
            .bind(("text", "z3rno's launch plan ships the Rust rewrite in Q4"))
            .bind(("embedding", query_embedding.clone()))
            .bind(("tenant", "acme"))
            .await?
            .check()?
            .take(0)
    })()
    .await;
    let mem_a = match mem_a {
        Ok(rows) if !rows.is_empty() => {
            report(
                "store memory record",
                true,
                format!("created {:?}", rows[0].id),
            );
            rows
        }
        Ok(_) => {
            report("store memory record", false, "CREATE returned no rows");
            return Ok(());
        }
        Err(e) => {
            report("store memory record", false, e.to_string());
            return Ok(());
        }
    };

    // second acme record, unrelated embedding, for the RELATE step below
    let mem_b: Vec<MemoryRecord> = acme
        .query("CREATE memory SET text = $text, embedding = $embedding, tenant = $tenant")
        .bind(("text", "z3rno's launch owner is the memory-engine squad"))
        .bind(("embedding", random_embedding(EMBED_DIM)))
        .bind(("tenant", "acme"))
        .await?
        .check()?
        .take(0)?;

    // --- 2. recall via native vector search ---------------------------------
    // Try the KNN operator first (needs the HNSW index above); fall back to
    // vector::similarity::cosine() if the operator/index combo rejects the
    // query in this server version - either is "native vector search".
    let knn = acme
        .query("SELECT id, text, tenant FROM memory WHERE embedding <|2,40|> $q")
        .bind(("q", query_embedding.clone()))
        .await?
        .check();
    match knn {
        Ok(mut r) => {
            let hits: Vec<MemoryRecord> = r.take(0).unwrap_or_default();
            let found = hits.iter().any(|h| h.id == mem_a[0].id);
            report(
                "vector recall (<|K,EF|> KNN operator)",
                found,
                format!("{} hit(s), top match is our record: {found}", hits.len()),
            );
        }
        Err(e) => {
            report(
                "vector recall (<|K,EF|> KNN operator)",
                false,
                format!("operator rejected: {e}; falling back"),
            );
            let cosine: Vec<MemoryRecord> = acme
                .query(
                    "SELECT id, text, tenant FROM memory \
                     ORDER BY vector::similarity::cosine(embedding, $q) DESC LIMIT 2",
                )
                .bind(("q", query_embedding.clone()))
                .await?
                .check()?
                .take(0)?;
            let found = cosine.iter().any(|h| h.id == mem_a[0].id);
            report(
                "vector recall (vector::similarity::cosine fallback)",
                found,
                format!("{} hit(s)", cosine.len()),
            );
        }
    }

    // --- 3. RELATE two records and traverse the graph edge back out --------
    let relate = acme
        .query("RELATE $a->relates_to->$b SET note = 'same launch'")
        .bind(("a", mem_a[0].id.clone()))
        .bind(("b", mem_b[0].id.clone()))
        .await?
        .check();
    match relate {
        Ok(_) => report(
            "RELATE graph edge",
            true,
            format!("{:?} -> relates_to -> {:?}", mem_a[0].id, mem_b[0].id),
        ),
        Err(e) => report("RELATE graph edge", false, e.to_string()),
    }

    let traverse: surrealdb::Result<Vec<RelatedText>> = acme
        .query("SELECT ->relates_to->memory.text AS related FROM $a")
        .bind(("a", mem_a[0].id.clone()))
        .await?
        .check()?
        .take(0)
        .map(|v: Vec<RelatedText>| v);
    match traverse {
        Ok(rows) => {
            let ok = rows.first().map(|r| !r.related.is_empty()).unwrap_or(false);
            report("graph traversal (-> arrow query)", ok, format!("{rows:?}"));
        }
        Err(e) => report("graph traversal (-> arrow query)", false, e.to_string()),
    }

    // --- 4. multi-tenant isolation: real permission enforcement, not app code ---
    let globex_mem: Vec<MemoryRecord> = globex
        .query("CREATE memory SET text = $text, embedding = $embedding, tenant = $tenant")
        .bind(("text", "globex's confidential roadmap"))
        .bind(("embedding", random_embedding(EMBED_DIM)))
        .bind(("tenant", "globex"))
        .await?
        .check()?
        .take(0)?;
    report(
        "tenant globex writes its own record",
        !globex_mem.is_empty(),
        format!("created {:?}", globex_mem.first().map(|r| &r.id)),
    );

    // acme reads the whole table - should see only its own rows, never globex's
    let acme_view: Vec<MemoryRecord> =
        acme.query("SELECT * FROM memory").await?.check()?.take(0)?;
    let leaks_globex = acme_view.iter().any(|r| r.tenant != "acme");
    report(
        "read isolation: acme cannot see globex rows",
        !leaks_globex && acme_view.len() == 2,
        format!(
            "acme sees {} row(s), all tenant=acme: {}",
            acme_view.len(),
            !leaks_globex
        ),
    );

    // acme tries to write a row tagged as globex - permission WHERE clause should reject it.
    // SurrealDB doesn't throw here: a CREATE that fails its PERMISSIONS check just
    // returns zero rows, same shape as a permission-scoped RLS policy in Postgres.
    // So the real assertion is "nothing landed", not "the query errored".
    let forged_write: surrealdb::Result<Vec<MemoryRecord>> = acme
        .query("CREATE memory SET text = 'forged', embedding = $embedding, tenant = 'globex'")
        .bind(("embedding", random_embedding(EMBED_DIM)))
        .await?
        .check()?
        .take(0);
    let forged_landed = matches!(&forged_write, Ok(rows) if !rows.is_empty());
    report(
        "write isolation: acme cannot forge a globex row",
        !forged_landed,
        match &forged_write {
            Ok(rows) if rows.is_empty() => "rejected: CREATE silently returned 0 rows".to_string(),
            Ok(rows) => format!("NOT rejected - forged row was written: {rows:?}"),
            Err(e) => format!("rejected with error: {e}"),
        },
    );

    // globex, symmetrically, cannot see acme's original record by id
    let globex_probe: Option<MemoryRecord> = globex
        .query("SELECT * FROM $id")
        .bind(("id", mem_a[0].id.clone()))
        .await?
        .check()?
        .take(0)?;
    report(
        "read isolation: globex cannot fetch acme's record by id",
        globex_probe.is_none(),
        format!("probe result: {globex_probe:?}"),
    );

    println!("== done ==");
    Ok(())
}
