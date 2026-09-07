use thiserror::Error;

/// Errors a storage backend can surface through any of the three trait
/// interfaces (`EngineBackend`, `VectorBackend`, `GraphBackend`).
#[derive(Debug, Error)]
pub enum BackendError {
    #[error("record not found: {0}")]
    NotFound(String),
    #[error("storage error: {0}")]
    Storage(#[source] anyhow::Error),
}

pub type BackendResult<T> = Result<T, BackendError>;

impl From<anyhow::Error> for BackendError {
    fn from(err: anyhow::Error) -> Self {
        BackendError::Storage(err)
    }
}
