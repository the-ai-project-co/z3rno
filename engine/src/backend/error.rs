use std::fmt;

use thiserror::Error;
use uuid::Uuid;

/// Which derived store a `forget` could not clean up yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupStep {
    Vector,
    Graph,
}

impl fmt::Display for CleanupStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CleanupStep::Vector => "vector",
            CleanupStep::Graph => "graph",
        })
    }
}

/// One derived-store cleanup that failed during `forget`, with the
/// underlying error rendered to text (the source error isn't `Clone`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupFailure {
    pub step: CleanupStep,
    pub message: String,
}

/// Stable, machine-readable reason a request was refused. Callers (CLI,
/// MCP, bindings, HTTP) branch on this, never on message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The input failed validation (shape, size, or a required field).
    InvalidInput,
    /// The requested status change isn't allowed from the current status.
    InvalidTransition,
    /// The caller's identity isn't allowed to perform this operation.
    NotPermitted,
    /// The id exists but isn't the kind of record the operation expects.
    WrongKind,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidInput => "invalid_input",
            ErrorCode::InvalidTransition => "invalid_transition",
            ErrorCode::NotPermitted => "not_permitted",
            ErrorCode::WrongKind => "wrong_kind",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors a storage backend can surface through any of the three trait
/// interfaces (`EngineBackend`, `VectorBackend`, `GraphBackend`), plus the
/// engine-level outcomes built on top of them.
#[derive(Debug, Error)]
pub enum BackendError {
    #[error("record not found: {0}")]
    NotFound(String),
    #[error("storage error: {0}")]
    Storage(#[source] anyhow::Error),
    /// The memory is already gone from the relational store (no read path
    /// returns it), but at least one derived store still holds its entry.
    /// The pending cleanup is durably recorded; calling `forget` again with
    /// the same id retries it.
    #[error(
        "forget of {memory_id} is incomplete ({}); call forget again to retry",
        failures.iter().map(|f| format!("{}: {}", f.step, f.message)).collect::<Vec<_>>().join("; ")
    )]
    ForgetIncomplete {
        memory_id: Uuid,
        failures: Vec<CleanupFailure>,
    },
    /// A request was refused for a domain reason (validation, lifecycle,
    /// permission) rather than a storage failure.
    #[error("{code}: {message}")]
    Rejected { code: ErrorCode, message: String },
}

impl BackendError {
    pub fn rejected(code: ErrorCode, message: impl Into<String>) -> Self {
        BackendError::Rejected {
            code,
            message: message.into(),
        }
    }

    /// The refusal code, if this is a `Rejected` error.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            BackendError::Rejected { code, .. } => Some(*code),
            _ => None,
        }
    }
}

pub type BackendResult<T> = Result<T, BackendError>;

impl From<anyhow::Error> for BackendError {
    fn from(err: anyhow::Error) -> Self {
        BackendError::Storage(err)
    }
}
