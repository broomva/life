//! Error surface for the lago-proxy crate.
//!
//! Sub-phase D adds [`RetryClass`] discrimination via [`LagoProxyError::retry_class`]
//! and the convenience [`LagoProxyError::is_retryable`].

use thiserror::Error;
use tonic::Code;

#[derive(Debug, Error)]
pub enum LagoProxyError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("substrate: {0}")]
    Substrate(tonic::Status),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

/// Retry classification for the lifed pool layer. Mirrors arcan-proxy's
/// taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    Retryable,
    Permanent,
}

impl LagoProxyError {
    pub fn retry_class(&self) -> RetryClass {
        match self {
            LagoProxyError::Transport(_) => RetryClass::Retryable,
            LagoProxyError::Substrate(s) => match s.code() {
                Code::Unavailable
                | Code::DeadlineExceeded
                | Code::Aborted
                | Code::ResourceExhausted => RetryClass::Retryable,
                _ => RetryClass::Permanent,
            },
            LagoProxyError::InvalidResponse(_) => RetryClass::Permanent,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self.retry_class(), RetryClass::Retryable)
    }
}

impl From<tonic::Status> for LagoProxyError {
    fn from(s: tonic::Status) -> Self {
        LagoProxyError::Substrate(s)
    }
}

impl From<LagoProxyError> for tonic::Status {
    fn from(e: LagoProxyError) -> Self {
        match e {
            LagoProxyError::Transport(m) => tonic::Status::unavailable(m),
            LagoProxyError::Substrate(s) => s,
            LagoProxyError::InvalidResponse(m) => tonic::Status::internal(m),
        }
    }
}

pub type LagoProxyResult<T> = Result<T, LagoProxyError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_exceeded_is_retryable() {
        let e = LagoProxyError::Substrate(tonic::Status::deadline_exceeded("slow"));
        assert!(e.is_retryable());
    }

    #[test]
    fn invalid_argument_is_permanent() {
        let e = LagoProxyError::Substrate(tonic::Status::invalid_argument("no"));
        assert_eq!(e.retry_class(), RetryClass::Permanent);
    }
}
