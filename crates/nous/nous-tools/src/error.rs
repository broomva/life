//! Errors emitted by the nous-tools lineage layer.
//!
//! Praxis tools surface their own errors through
//! [`aios_protocol::tool::ToolError`]; this module is dedicated to the
//! [`crate::NousLineage`] trait, which has its own append/query
//! semantics distinct from tool dispatch.

use thiserror::Error;

/// Errors that can occur when recording or querying a [`crate::NousLineage`].
#[derive(Debug, Error)]
pub enum NousLineageError {
    /// The backing store rejected the write (e.g. lock poisoning,
    /// downstream lago error).
    #[error("lineage store error: {0}")]
    Store(String),

    /// The provided filter was malformed (e.g. inverted time window).
    #[error("invalid lineage filter: {0}")]
    InvalidFilter(String),

    /// Unexpected internal failure (kept open-ended so backends can
    /// surface their own error messages without growing this enum).
    #[error("internal lineage error: {0}")]
    Internal(String),
}
