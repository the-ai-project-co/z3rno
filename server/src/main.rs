//! The `z3rno-server` binary: parses config, wires up `AppState`, and
//! calls `z3rno_server::run`. Gated behind the `bin` feature (see
//! Cargo.toml) so library-only consumers don't pay for `clap`/`dotenvy`.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use z3rno_engine::MemoryEngine;
use z3rno_server::auth::AuthConfig;
use z3rno_server::cache::{CacheBackend, SqliteCacheBackend};
use z3rno_server::{observability, run, AppState};

#[derive(Parser)]
#[command(name = "z3rno-server", about = "The z3rno HTTP API server")]
struct Args {
    /// Address to bind, e.g. 0.0.0.0:8080.
    #[arg(long, env = "Z3RNO_LISTEN_ADDR", default_value = "0.0.0.0:8080")]
    listen_addr: SocketAddr,

    /// Postgres connection string for the production backend. Omit to
    /// run against the embedded backend instead (--sqlite-path).
    #[arg(long, env = "Z3RNO_DATABASE_URL")]
    database_url: Option<String>,

    /// SQLite file for the embedded engine backend, used when
    /// --database-url is not set.
    #[arg(long, env = "Z3RNO_SQLITE_PATH", default_value = "z3rno.db")]
    sqlite_path: String,

    /// SQLite file for the cache backend (sessions, auth verification
    /// cache, admin budgets). Separate from the engine's own storage.
    #[arg(long, env = "Z3RNO_CACHE_SQLITE_PATH", default_value = "z3rno-cache.db")]
    cache_sqlite_path: String,

    /// Secret used to sign/verify JWTs. Required — the server refuses to
    /// start without one rather than falling back to an insecure default.
    #[arg(long, env = "Z3RNO_JWT_SECRET")]
    jwt_secret: String,

    /// Enables the superadmin cross-tenant budget endpoints when set,
    /// matching the old system's superadmin_enabled + superadmin_api_key
    /// pair (a request bearing this key gets role = "superadmin").
    #[arg(long, env = "Z3RNO_SUPERADMIN_API_KEY")]
    superadmin_api_key: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    observability::init_tracing();
    let args = Args::parse();

    let engine = match &args.database_url {
        Some(url) => MemoryEngine::postgres(url).await?,
        None => MemoryEngine::embedded(&args.sqlite_path)?,
    };

    let cache: Arc<dyn CacheBackend> = Arc::new(SqliteCacheBackend::open(&args.cache_sqlite_path)?);

    let state = AppState {
        engine: Arc::new(engine),
        cache,
        auth: Arc::new(AuthConfig {
            jwt_secret: args.jwt_secret,
            superadmin_api_key: args.superadmin_api_key,
        }),
    };

    run(args.listen_addr, state).await
}
