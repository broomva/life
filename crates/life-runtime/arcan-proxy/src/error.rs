//! Error surface for the arcan-proxy crate.
//!
//! Sub-phase D adds [`RetryClass`] discrimination via [`ArcanProxyError::retry_class`]
//! and the convenience [`ArcanProxyError::is_retryable`]. The lifed pool layer
//! reads this so transient transport / `Unavailable` faults retry via the
//! breaker while permanent (`InvalidArgument`, `PermissionDenied`,
//! `Unauthenticated`, `NotFound`, `AlreadyExists`, `FailedPrecondition`)
//! statuses fail fast. Per Spec C₂ §7.2.

use thiserror::Error;
use tonic::Code;

#[derive(Debug, Error)]
pub enum ArcanProxyError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("substrate: {0}")]
    Substrate(tonic::Status),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

/// Retry classification for the lifed pool layer. `Retryable` faults
/// trip the circuit breaker on accumulation but the pool retries on
/// transient hiccups; `Permanent` faults fail fast.
///
/// `#[non_exhaustive]` — additional classes (e.g., `RetryWithBackoff`
/// for ResourceExhausted) may be added without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetryClass {
    Retryable,
    Permanent,
}

impl ArcanProxyError {
    /// Per Spec C₂ §7.2: `Unavailable`, `DeadlineExceeded`, `Aborted`, and
    /// transport errors retry; everything else is permanent.
    pub fn retry_class(&self) -> RetryClass {
        match self {
            ArcanProxyError::Transport(_) => RetryClass::Retryable,
            ArcanProxyError::Substrate(s) => match s.code() {
                Code::Unavailable
                | Code::DeadlineExceeded
                | Code::Aborted
                | Code::ResourceExhausted => RetryClass::Retryable,
                _ => RetryClass::Permanent,
            },
            ArcanProxyError::InvalidResponse(_) => RetryClass::Permanent,
        }
    }

    /// True iff [`Self::retry_class`] is [`RetryClass::Retryable`].
    pub fn is_retryable(&self) -> bool {
        matches!(self.retry_class(), RetryClass::Retryable)
    }
}

impl From<tonic::Status> for ArcanProxyError {
    fn from(s: tonic::Status) -> Self {
        ArcanProxyError::Substrate(s)
    }
}

impl From<ArcanProxyError> for tonic::Status {
    fn from(e: ArcanProxyError) -> Self {
        match e {
            ArcanProxyError::Transport(m) => tonic::Status::unavailable(m),
            ArcanProxyError::Substrate(s) => s,
            ArcanProxyError::InvalidResponse(m) => tonic::Status::internal(m),
        }
    }
}

pub type ArcanProxyResult<T> = Result<T, ArcanProxyError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_is_retryable() {
        assert!(ArcanProxyError::Transport("conn refused".into()).is_retryable());
    }

    #[test]
    fn unavailable_substrate_is_retryable() {
        let e = ArcanProxyError::Substrate(tonic::Status::unavailable("down"));
        assert!(e.is_retryable());
    }

    #[test]
    fn permission_denied_is_permanent() {
        let e = ArcanProxyError::Substrate(tonic::Status::permission_denied("no"));
        assert_eq!(e.retry_class(), RetryClass::Permanent);
    }

    #[test]
    fn invalid_response_is_permanent() {
        assert_eq!(
            ArcanProxyError::InvalidResponse("bad".into()).retry_class(),
            RetryClass::Permanent,
        );
    }
}
