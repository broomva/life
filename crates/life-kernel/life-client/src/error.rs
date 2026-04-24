//! Error types for the Life Kernel client.

use thiserror::Error;

/// Errors raised by the `life-client` crate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LifeClientError {
    /// Transport-level failure (connection refused, timeout, etc.).
    #[error("transport: {0}")]
    Transport(String),
    /// RPC-level error returned by the server.
    #[error("rpc: {0}")]
    Rpc(String),
}

/// Convenience result type for `life-client` operations.
pub type LifeResult<T> = Result<T, LifeClientError>;
