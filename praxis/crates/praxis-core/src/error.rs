//! Error types for praxis-core.

use thiserror::Error;

/// Errors from the Praxis core infrastructure.
#[derive(Debug, Error)]
pub enum PraxisError {
    #[error("workspace policy violation: {0}")]
    WorkspaceViolation(String),

    #[error("sandbox error: {0}")]
    Sandbox(String),

    #[error("command execution failed: {0}")]
    CommandFailed(String),

    #[error("path not within workspace: {path}")]
    PathOutsideWorkspace { path: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience result type for Praxis operations.
pub type PraxisResult<T> = Result<T, PraxisError>;
