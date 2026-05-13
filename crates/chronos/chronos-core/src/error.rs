//! Crate-local error type.

use thiserror::Error;

/// Errors produced by chronos primitives.
///
/// `Trigger` wraps source-specific failures (e.g. a cron parse error in M3+). `Router` is
/// reserved for routing-level invariant violations the router itself surfaces.
#[derive(Debug, Error)]
pub enum ChronosError {
    /// A trigger failed to produce events (descriptive string for now; a real error type can
    /// be added once concrete trigger sources are implemented in M1+).
    #[error("chronos trigger error: {0}")]
    Trigger(String),

    /// The router observed an invariant violation.
    #[error("chronos router error: {0}")]
    Router(String),
}

/// Result alias used throughout chronos-core.
pub type ChronosResult<T> = std::result::Result<T, ChronosError>;
