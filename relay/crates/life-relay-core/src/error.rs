//! Relay error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("connection failed: {0}")]
    Connection(String),

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session spawn failed: {0}")]
    SpawnFailed(String),

    #[error("adapter error: {0}")]
    Adapter(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type RelayResult<T> = Result<T, RelayError>;
