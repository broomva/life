//! Error type for [`crate::InferenceBackend`] operations.

use crate::types::CloseCode;

/// Top-level error type returned by inference operations.
///
/// `#[non_exhaustive]` because the spec reserves the right to add
/// variants in minor releases; downstream code must use a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InferenceError {
    /// The backend rejected or aborted the call. Carries a Spec-E
    /// close code and a human-readable message. Most non-network
    /// errors flow through here.
    #[error("backend error ({code:?}): {message}")]
    Backend {
        /// Close code categorising the failure mode.
        code: CloseCode,
        /// Human-readable message for logs / UI.
        message: String,
    },

    /// Transport-level I/O error. Use [`InferenceError::Backend`] with
    /// [`CloseCode::BackendUnavailable`] for higher-level routing.
    #[error("network error: {0}")]
    Network(#[from] std::io::Error),

    /// Caller dropped the future before completion.
    #[error("cancelled")]
    Cancelled,
}

impl InferenceError {
    /// Construct a [`InferenceError::Backend`] with a `String` message.
    #[must_use]
    pub fn backend(code: CloseCode, message: impl Into<String>) -> Self {
        Self::Backend {
            code,
            message: message.into(),
        }
    }

    /// Returns the [`CloseCode`] carried by [`InferenceError::Backend`],
    /// or `None` for other variants.
    #[must_use]
    pub fn close_code(&self) -> Option<CloseCode> {
        match self {
            Self::Backend { code, .. } => Some(*code),
            _ => None,
        }
    }
}
