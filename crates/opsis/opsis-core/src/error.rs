use thiserror::Error;

/// Errors produced by the Opsis world state engine.
#[derive(Debug, Error)]
pub enum OpsisError {
    /// A feed failed to connect or poll.
    #[error("feed error: {0}")]
    Feed(String),

    /// Serialization / deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Invalid spatial data (coordinates out of range, etc.).
    #[error("spatial error: {0}")]
    Spatial(String),

    /// Subscription matching or registration error.
    #[error("subscription error: {0}")]
    Subscription(String),

    /// The engine is not running or has shut down.
    #[error("engine not running")]
    EngineNotRunning,
}

/// Convenience alias used throughout the crate.
pub type OpsisResult<T> = Result<T, OpsisError>;
