//! Crate-level error type.
//!
//! Following the workspace convention: `thiserror` in libraries.

use thiserror::Error;

/// Errors that arise while injecting, measuring, or fitting perturbations.
#[derive(Debug, Error)]
pub enum PerturbError {
    /// The requested perturbation is not yet implemented at this level.
    #[error("perturbation not yet implemented: level={level:?} kind={kind}")]
    NotImplemented {
        level: crate::perturbation::Level,
        kind: &'static str,
    },

    /// The injector failed to apply or revert the perturbation.
    #[error("injector failure: {0}")]
    Injector(String),

    /// The recovery fit could not be produced (e.g. too few samples).
    #[error("fit failure: {0}")]
    Fit(String),

    /// Underlying I/O or telemetry transport error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Crate-level result alias.
pub type PerturbResult<T> = Result<T, PerturbError>;
