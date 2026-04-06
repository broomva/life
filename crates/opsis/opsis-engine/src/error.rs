//! Engine-level errors for the Opsis world state engine.

use opsis_core::OpsisError;
use thiserror::Error;

/// Errors produced by the Opsis engine runtime.
#[derive(Debug, Error)]
pub enum EngineError {
    /// Error from the core types layer.
    #[error("core error: {0}")]
    Core(#[from] OpsisError),

    /// A feed failed to connect or ingest data.
    #[error("feed error [{feed_name}]: {message}")]
    Feed {
        /// Which feed produced the error.
        feed_name: String,
        /// Human-readable description.
        message: String,
    },

    /// Error in the SSE stream layer.
    #[error("stream error: {0}")]
    Stream(String),

    /// IO error (filesystem, network, etc.).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// HTTP client error from reqwest.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// Configuration error (e.g. invalid feeds.toml).
    #[error("config error: {0}")]
    Config(String),
}

/// Convenience alias used throughout the engine crate.
pub type EngineResult<T> = Result<T, EngineError>;
