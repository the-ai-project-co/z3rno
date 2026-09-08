//! Zero-infra default backends: embedded, in-process, no external services.

pub(crate) mod conn;
pub mod graph;
pub mod sqlite;
pub mod vector;

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use super::error::BackendResult;

/// Opens the one SQLite connection `MemoryEngine::embedded` hands to the
/// relational, vector, and graph backends, so all three share a single
/// connection to the file rather than each opening their own.
pub(crate) fn open_shared_connection<P: AsRef<Path>>(
    path: P,
) -> BackendResult<Arc<Mutex<Connection>>> {
    conn::open(path)
}
