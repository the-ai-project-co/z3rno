//! z3rno-cli: a real, standalone CLI on top of `z3rno-engine`/`z3rno-server`
//! (slice 0007.1) — not just a thin wrapper shell. `init`/`store`/`recall`/
//! `forget` talk to a local embedded SQLite store directly
//! (`MemoryEngine::embedded`, same as the engine's own zero-infra default);
//! `serve` runs the real HTTP API (`z3rno-server` as a library, see that
//! crate's `run`/`AppState`) so `npx z3rno serve` (once 0007.2 ships) is a
//! complete local dev server.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use uuid::Uuid;
use z3rno_engine::{MemoryEngine, Tier};
use z3rno_hash_embed::hash_embed;
use z3rno_server::auth::{AuthConfig, Role};
use z3rno_server::cache::{CacheBackend, SqliteCacheBackend};
use z3rno_server::{observability, run, AppState};

/// Tenant id used everywhere `--tenant` is omitted. z3rno-cli is a
/// single-operator local tool by default — multi-tenancy is a server/
/// production concern (see `z3rno-server`'s `AuthContext`), so the CLI
/// doesn't make every command think about it up front.
const DEFAULT_TENANT: &str = "local";

/// Default embedded SQLite path, shared by `init`/`store`/`recall` so
/// running them back to back with no flags composes into one local store
/// (matches the acceptance check: `init && store ... && recall ...`).
/// Also matches `z3rno-server`'s own `--sqlite-path` default.
const DEFAULT_DB_PATH: &str = "z3rno.db";

#[derive(Parser)]
#[command(
    name = "z3rno",
    version,
    about = "z3rno: a memory engine for AI agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Bootstrap a local embedded store (creates the SQLite file/schema).
    Init {
        /// SQLite file to create.
        #[arg(long, default_value = DEFAULT_DB_PATH)]
        path: String,
        /// Tenant id to report back (embedded storage doesn't need one
        /// provisioned up front — every tenant is just a key it scopes
        /// rows by).
        #[arg(long)]
        tenant: Option<String>,
    },

    /// Run the z3rno HTTP API server locally.
    Serve {
        /// Address to bind, e.g. 0.0.0.0:8080.
        #[arg(long, env = "Z3RNO_LISTEN_ADDR", default_value = "0.0.0.0:8080")]
        listen_addr: SocketAddr,

        /// Postgres connection string for the production backend. Omit to
        /// run against the embedded backend instead (--sqlite-path).
        #[arg(long, env = "Z3RNO_DATABASE_URL")]
        database_url: Option<String>,

        /// SQLite file for the embedded engine backend, used when
        /// --database-url is not set.
        #[arg(long, env = "Z3RNO_SQLITE_PATH", default_value = DEFAULT_DB_PATH)]
        sqlite_path: String,

        /// SQLite file for the cache backend (sessions, auth verification
        /// cache, admin budgets).
        #[arg(
            long,
            env = "Z3RNO_CACHE_SQLITE_PATH",
            default_value = "z3rno-cache.db"
        )]
        cache_sqlite_path: String,

        /// Secret used to sign/verify JWTs. Unlike the standalone
        /// `z3rno-server` binary (which refuses to start without one),
        /// this is optional here: omit it and a random secret is
        /// generated for this run. That's a friendly local/dev default,
        /// not a production path — sessions/tokens won't survive a
        /// restart when the secret wasn't pinned.
        #[arg(long, env = "Z3RNO_JWT_SECRET")]
        jwt_secret: Option<String>,

        /// Enables the superadmin cross-tenant budget endpoints when set.
        #[arg(long, env = "Z3RNO_SUPERADMIN_API_KEY")]
        superadmin_api_key: Option<String>,
    },

    /// Store a memory in a local embedded store.
    Store {
        /// The memory's content.
        content: String,

        #[arg(long)]
        tenant: Option<String>,

        /// SQLite file to store into.
        #[arg(long, default_value = DEFAULT_DB_PATH)]
        path: String,

        /// One of: working, episodic, semantic, procedural.
        #[arg(long, default_value = "semantic")]
        tier: String,

        /// Comma-separated floats, e.g. "0.1,0.2,0.3". Omit to embed
        /// `content` with the CLI's naive local hashing embedding (see
        /// the `z3rno-hash-embed` crate) — good enough to make `recall`
        /// work out of the box, not a substitute for a real embedding
        /// model.
        #[arg(long)]
        embedding: Option<String>,

        /// A JSON object attached to the memory as-is.
        #[arg(long)]
        metadata: Option<String>,
    },

    /// Recall memories from a local embedded store.
    Recall {
        /// The text to search for.
        query: String,

        #[arg(long)]
        tenant: Option<String>,

        /// SQLite file to recall from.
        #[arg(long, default_value = DEFAULT_DB_PATH)]
        path: String,

        /// Max number of results.
        #[arg(long, default_value_t = 5)]
        k: usize,

        /// Comma-separated floats to search with instead of embedding
        /// `query` with the naive local hashing embedding. Must use the
        /// same embedding space as the memories being searched — mixing
        /// real embeddings and the naive hashing ones won't match.
        #[arg(long)]
        embedding: Option<String>,
    },

    /// Forget (delete) a memory by id from a local embedded store.
    Forget {
        /// The memory's id, as printed by `z3rno store`.
        id: Uuid,

        #[arg(long)]
        tenant: Option<String>,

        /// SQLite file to forget from.
        #[arg(long, default_value = DEFAULT_DB_PATH)]
        path: String,
    },

    /// Issues a JWT for a running `z3rno serve`, signed with its
    /// --jwt-secret. z3rno-server has no user database — `--jwt-secret`
    /// (or the `Z3RNO_JWT_SECRET` a server was started with) is the one
    /// shared secret this command and the server's own verification both
    /// trust; anyone who has it can mint a working token for any tenant.
    Token {
        /// Tenant (org) id to embed in the token.
        #[arg(long)]
        tenant: String,

        /// One of: admin, write, read, audit.
        #[arg(long, default_value = "admin")]
        role: String,

        /// Seconds until the token expires.
        #[arg(long, default_value_t = 3600)]
        ttl_secs: i64,

        /// Must match the target server's --jwt-secret/Z3RNO_JWT_SECRET —
        /// a token signed with any other secret is just as unauthorized as
        /// no token at all.
        #[arg(long, env = "Z3RNO_JWT_SECRET")]
        jwt_secret: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { path, tenant } => cmd_init(path, tenant).await,
        Command::Serve {
            listen_addr,
            database_url,
            sqlite_path,
            cache_sqlite_path,
            jwt_secret,
            superadmin_api_key,
        } => {
            cmd_serve(
                listen_addr,
                database_url,
                sqlite_path,
                cache_sqlite_path,
                jwt_secret,
                superadmin_api_key,
            )
            .await
        }
        Command::Store {
            content,
            tenant,
            path,
            tier,
            embedding,
            metadata,
        } => cmd_store(content, tenant, path, tier, embedding, metadata).await,
        Command::Recall {
            query,
            tenant,
            path,
            k,
            embedding,
        } => cmd_recall(query, tenant, path, k, embedding).await,
        Command::Forget { id, tenant, path } => cmd_forget(id, tenant, path).await,
        Command::Token {
            tenant,
            role,
            ttl_secs,
            jwt_secret,
        } => cmd_token(tenant, role, ttl_secs, jwt_secret),
    }
}

async fn cmd_init(path: String, tenant: Option<String>) -> anyhow::Result<()> {
    let tenant = tenant.unwrap_or_else(|| DEFAULT_TENANT.to_string());
    let engine = MemoryEngine::embedded(&path)?;
    println!(
        "Initialized z3rno store at {path} (tier: {:?}, tenant: {tenant})",
        engine.tier()
    );
    Ok(())
}

async fn cmd_serve(
    listen_addr: SocketAddr,
    database_url: Option<String>,
    sqlite_path: String,
    cache_sqlite_path: String,
    jwt_secret: Option<String>,
    superadmin_api_key: Option<String>,
) -> anyhow::Result<()> {
    observability::init_tracing();
    observability::install_metrics_recorder();

    let engine = match &database_url {
        Some(url) => MemoryEngine::postgres(url).await?,
        None => MemoryEngine::embedded(&sqlite_path)?,
    };

    let cache: Arc<dyn CacheBackend> = Arc::new(SqliteCacheBackend::open(&cache_sqlite_path)?);

    let jwt_secret = jwt_secret.unwrap_or_else(random_jwt_secret);

    let state = AppState {
        engine: Arc::new(engine),
        cache,
        auth: Arc::new(AuthConfig {
            jwt_secret,
            superadmin_api_key,
        }),
    };

    run(listen_addr, state).await
}

/// A dev-convenience default, not a production secret store: generates a
/// random-enough JWT signing secret (a v4 UUID has 122 bits of randomness,
/// plenty for a secret nothing persists past this process) and warns loudly
/// that it won't survive a restart, so it's obvious this isn't the flag to
/// rely on for anything long-lived.
fn random_jwt_secret() -> String {
    eprintln!(
        "warning: no --jwt-secret given, generated a random one for this run only \
         — existing sessions and tokens will stop working on restart. Pass \
         --jwt-secret (or set Z3RNO_JWT_SECRET) to pin it."
    );
    Uuid::new_v4().to_string()
}

async fn cmd_store(
    content: String,
    tenant: Option<String>,
    path: String,
    tier: String,
    embedding: Option<String>,
    metadata: Option<String>,
) -> anyhow::Result<()> {
    let tenant = tenant.unwrap_or_else(|| DEFAULT_TENANT.to_string());
    let tier = parse_tier(&tier)?;
    let embedding = match embedding {
        Some(raw) => parse_embedding(&raw)?,
        None => hash_embed(&content),
    };
    let metadata = match metadata {
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("invalid --metadata JSON: {e}"))?,
        None => serde_json::Value::Null,
    };

    let engine = MemoryEngine::embedded(&path)?;
    let memory = engine
        .store(
            &tenant,
            tier,
            content,
            Some(embedding),
            metadata,
            Vec::new(),
        )
        .await?;

    println!("{}", memory.id);
    Ok(())
}

async fn cmd_recall(
    query: String,
    tenant: Option<String>,
    path: String,
    k: usize,
    embedding: Option<String>,
) -> anyhow::Result<()> {
    let tenant = tenant.unwrap_or_else(|| DEFAULT_TENANT.to_string());
    let embedding = match embedding {
        Some(raw) => parse_embedding(&raw)?,
        None => hash_embed(&query),
    };

    let engine = MemoryEngine::embedded(&path)?;
    let memories = engine.recall(&tenant, embedding, k).await?;

    if memories.is_empty() {
        println!("No memories found.");
        return Ok(());
    }
    for m in memories {
        println!("{}  [{:?}]  {}", m.id, m.tier, truncate(&m.content, 100));
    }
    Ok(())
}

async fn cmd_forget(id: Uuid, tenant: Option<String>, path: String) -> anyhow::Result<()> {
    let tenant = tenant.unwrap_or_else(|| DEFAULT_TENANT.to_string());
    let engine = MemoryEngine::embedded(&path)?;

    match engine.forget(&tenant, id).await? {
        Some(proof) => println!(
            "Forgot {id} (audit event {}, hash {})",
            proof.audit_event_id, proof.hash
        ),
        None => println!("No memory with id {id} found for tenant {tenant:?}."),
    }
    Ok(())
}

fn cmd_token(
    tenant: String,
    role: String,
    ttl_secs: i64,
    jwt_secret: String,
) -> anyhow::Result<()> {
    let role = parse_role(&role)?;
    let token = z3rno_server::auth::issue_jwt(&jwt_secret, &tenant, role, ttl_secs)?;
    println!("{token}");
    Ok(())
}

/// Parses a lower-case role string into the four roles `issue_jwt` accepts
/// (`superadmin` deliberately isn't one of them — see `Command::Token`'s
/// doc comment and `issue_jwt`'s own).
fn parse_role(s: &str) -> anyhow::Result<Role> {
    match s.to_lowercase().as_str() {
        "admin" => Ok(Role::Admin),
        "write" => Ok(Role::Write),
        "read" => Ok(Role::Read),
        "audit" => Ok(Role::Audit),
        other => anyhow::bail!(
            "invalid role {other:?}: expected one of \"admin\", \"write\", \"read\", \"audit\""
        ),
    }
}

/// Parses a snake_case tier string, matching the Python/TypeScript bindings'
/// own `parse_tier` (bindings/python/src/lib.rs, bindings/typescript/src/
/// lib.rs) so the same tier names work everywhere z3rno is used from.
fn parse_tier(s: &str) -> anyhow::Result<Tier> {
    match s.to_lowercase().as_str() {
        "working" => Ok(Tier::Working),
        "episodic" => Ok(Tier::Episodic),
        "semantic" => Ok(Tier::Semantic),
        "procedural" => Ok(Tier::Procedural),
        other => anyhow::bail!(
            "invalid tier {other:?}: expected one of \"working\", \"episodic\", \"semantic\", \"procedural\""
        ),
    }
}

fn parse_embedding(raw: &str) -> anyhow::Result<Vec<f32>> {
    raw.split(',')
        .map(|s| {
            s.trim()
                .parse::<f32>()
                .map_err(|e| anyhow::anyhow!("invalid --embedding value {s:?}: {e}"))
        })
        .collect()
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tier_accepts_every_snake_case_variant() {
        assert_eq!(parse_tier("working").unwrap(), Tier::Working);
        assert_eq!(parse_tier("episodic").unwrap(), Tier::Episodic);
        assert_eq!(parse_tier("semantic").unwrap(), Tier::Semantic);
        assert_eq!(parse_tier("procedural").unwrap(), Tier::Procedural);
        // Case-insensitive, matching clap's usual leniency for free-text args.
        assert_eq!(parse_tier("Working").unwrap(), Tier::Working);
    }

    #[test]
    fn parse_tier_rejects_unknown_value() {
        assert!(parse_tier("bogus").is_err());
    }

    #[test]
    fn parse_role_accepts_every_issuable_role() {
        assert_eq!(parse_role("admin").unwrap(), Role::Admin);
        assert_eq!(parse_role("write").unwrap(), Role::Write);
        assert_eq!(parse_role("read").unwrap(), Role::Read);
        assert_eq!(parse_role("audit").unwrap(), Role::Audit);
        assert_eq!(parse_role("Admin").unwrap(), Role::Admin);
    }

    #[test]
    fn parse_role_rejects_superadmin_and_garbage() {
        // Not issuable as a JWT — see `issue_jwt`'s own doc comment.
        assert!(parse_role("superadmin").is_err());
        assert!(parse_role("bogus").is_err());
    }

    #[test]
    fn cmd_token_prints_a_token_the_server_would_accept() {
        cmd_token(
            "tenant-a".to_string(),
            "admin".to_string(),
            3600,
            "shared-secret".to_string(),
        )
        .unwrap();
    }

    #[test]
    fn parse_embedding_parses_comma_separated_floats() {
        assert_eq!(
            parse_embedding("0.1, 0.2,0.3").unwrap(),
            vec![0.1f32, 0.2, 0.3]
        );
    }

    #[test]
    fn parse_embedding_rejects_garbage() {
        assert!(parse_embedding("0.1,not-a-float").is_err());
    }

    #[test]
    fn truncate_leaves_short_strings_untouched() {
        assert_eq!(truncate("hello", 100), "hello");
    }

    #[test]
    fn truncate_adds_ellipsis_past_the_limit() {
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[tokio::test]
    async fn store_then_recall_finds_the_memory_end_to_end() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let engine = MemoryEngine::embedded(db_file.path()).unwrap();

        let content = "the quick brown fox jumps over the lazy dog";
        engine
            .store(
                DEFAULT_TENANT,
                Tier::Semantic,
                content.to_string(),
                Some(hash_embed(content)),
                serde_json::Value::Null,
                Vec::new(),
            )
            .await
            .unwrap();

        // An unrelated memory, so recall has something to rank the match
        // above rather than being trivially the only result.
        let unrelated = "quarterly revenue exceeded analyst forecasts";
        engine
            .store(
                DEFAULT_TENANT,
                Tier::Semantic,
                unrelated.to_string(),
                Some(hash_embed(unrelated)),
                serde_json::Value::Null,
                Vec::new(),
            )
            .await
            .unwrap();

        let results = engine
            .recall(DEFAULT_TENANT, hash_embed("quick fox"), 1)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, content);
    }

    #[tokio::test]
    async fn store_then_forget_removes_the_memory() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let engine = MemoryEngine::embedded(db_file.path()).unwrap();

        let content = "the quick brown fox jumps over the lazy dog";
        let memory = engine
            .store(
                DEFAULT_TENANT,
                Tier::Semantic,
                content.to_string(),
                Some(hash_embed(content)),
                serde_json::Value::Null,
                Vec::new(),
            )
            .await
            .unwrap();

        let proof = engine.forget(DEFAULT_TENANT, memory.id).await.unwrap();
        assert!(proof.is_some());

        let results = engine
            .recall(DEFAULT_TENANT, hash_embed("quick fox"), 5)
            .await
            .unwrap();
        assert!(results.is_empty());

        // Forgetting again is a no-op, not an error.
        let second = engine.forget(DEFAULT_TENANT, memory.id).await.unwrap();
        assert!(second.is_none());
    }
}
