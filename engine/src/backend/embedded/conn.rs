//! One shared, mutex-guarded SQLite connection for every embedded backend
//! (relational, vector, graph), so `MemoryEngine::embedded` opens exactly
//! one connection to the file rather than three independently-locking
//! ones. Each backend owns its own tables inside it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use super::super::error::BackendResult;

pub(super) fn open<P: AsRef<Path>>(path: P) -> BackendResult<Arc<Mutex<Connection>>> {
    let conn = Connection::open(path).map_err(anyhow::Error::from)?;
    Ok(Arc::new(Mutex::new(conn)))
}

pub(super) fn open_in_memory() -> BackendResult<Arc<Mutex<Connection>>> {
    let conn = Connection::open_in_memory().map_err(anyhow::Error::from)?;
    Ok(Arc::new(Mutex::new(conn)))
}
