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

    /// An agenda operation referenced an item id that does not exist in the store.
    #[error("chronos agenda item not found: {0}")]
    NotFound(String),

    /// The agenda backing store failed (e.g. a lago journal append/read error in
    /// `chronos-lago::LagoAgendaStore`). Carries the backend error rendered as a string so
    /// `chronos-core` stays free of a `lago-core` dependency.
    #[error("chronos agenda store error: {0}")]
    Agenda(String),
}

/// Result alias used throughout chronos-core.
pub type ChronosResult<T> = std::result::Result<T, ChronosError>;
