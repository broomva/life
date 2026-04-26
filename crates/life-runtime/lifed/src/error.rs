//! Error surface for the `lifed` daemon.
//!
//! Every fallible subsystem result is converted into [`LifedError`] at
//! the boundary so daemon startup failures surface with a single error
//! type (printed to stderr and to the systemd journal).
//!
//! At the RPC boundary, [`LifedError`] also implements
//! `Into<tonic::Status>` so handlers can `?`-bubble through the auth +
//! routing layers and have errors mapped to canonical gRPC codes.

use thiserror::Error;
use tonic::Status;

/// All errors surfaced by the lifed daemon.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LifedError {
    /// Configuration loading or validation failed.
    #[error("configuration: {0}")]
    Config(String),

    /// Auth subsystem failure (JWKS load, signature verification, blocklist).
    #[error("auth: {0}")]
    Auth(String),

    /// Routing cache failure (cold-start replay, eviction, lookup).
    #[error("routing: {0}")]
    Routing(String),

    /// Saga orchestration failure (forward step, compensation, deadline).
    #[error("saga: {0}")]
    Saga(String),

    /// Substrate dispatch failure (transport, breaker open, semaphore).
    #[error("substrate: {0}")]
    Substrate(String),

    /// Listener bind / accept / chmod failure.
    #[error("listener: {0}")]
    Listener(String),

    /// Server-level failure (tonic Server::serve, drain).
    #[error("server: {0}")]
    Server(String),

    /// Shutdown drain timeout or signal-handler error.
    #[error("shutdown: {0}")]
    Shutdown(String),
}

/// Convenience alias used throughout the daemon.
pub type LifedResult<T> = Result<T, LifedError>;

impl From<LifedError> for Status {
    fn from(err: LifedError) -> Self {
        match err {
            LifedError::Auth(m) => Status::unauthenticated(m),
            LifedError::Routing(m) => Status::not_found(m),
            LifedError::Saga(m) => Status::aborted(m),
            LifedError::Substrate(m) => Status::unavailable(m),
            LifedError::Config(m)
            | LifedError::Listener(m)
            | LifedError::Server(m)
            | LifedError::Shutdown(m) => Status::internal(m),
        }
    }
}
