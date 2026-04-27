//! Error surface for the `lifegw` daemon.
//!
//! Every fallible subsystem result is converted into [`LifegwError`] at
//! the boundary so daemon startup failures surface with a single error
//! type (printed to stderr and to the systemd journal).
//!
//! At the RPC boundary, [`LifegwError`] also implements `Into<tonic::Status>`
//! so handlers can `?`-bubble through the proxy layer and have errors mapped
//! to canonical gRPC codes.

use thiserror::Error;
use tonic::Status;

/// All errors surfaced by the lifegw daemon.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LifegwError {
    /// Configuration loading or validation failed.
    #[error("configuration: {0}")]
    Config(String),

    /// Auth subsystem failure (JWT verification, Tier-2 mint).
    #[error("auth: {0}")]
    Auth(String),

    /// TLS subsystem failure (cert load, key load, ServerConfig build).
    #[error("tls: {0}")]
    Tls(String),

    /// Listener bind / accept failure.
    #[error("listener: {0}")]
    Listener(String),

    /// Upstream lifed dial failure.
    #[error("upstream: {0}")]
    Upstream(String),

    /// Proxy forwarding failure.
    #[error("proxy: {0}")]
    Proxy(String),

    /// Server-level failure (tonic Server::serve, drain).
    #[error("server: {0}")]
    Server(String),

    /// Shutdown drain timeout or signal-handler error.
    #[error("shutdown: {0}")]
    Shutdown(String),
}

/// Convenience alias used throughout the daemon.
pub type LifegwResult<T> = Result<T, LifegwError>;

impl From<LifegwError> for Status {
    fn from(err: LifegwError) -> Self {
        match err {
            LifegwError::Auth(m) => Status::unauthenticated(m),
            LifegwError::Upstream(m) | LifegwError::Proxy(m) => Status::unavailable(m),
            LifegwError::Config(m)
            | LifegwError::Tls(m)
            | LifegwError::Listener(m)
            | LifegwError::Server(m)
            | LifegwError::Shutdown(m) => Status::internal(m),
        }
    }
}
