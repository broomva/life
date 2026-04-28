//! Error surface for the anima-proxy crate.
//!
//! Sub-phase D adds [`RetryClass`] discrimination via [`AnimaProxyError::retry_class`]
//! and the convenience [`AnimaProxyError::is_retryable`].

use thiserror::Error;
use tonic::Code;

#[derive(Debug, Error)]
pub enum AnimaProxyError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("substrate: {0}")]
    Substrate(tonic::Status),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

/// Retry classification for the lifed pool layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    Retryable,
    Permanent,
}

impl AnimaProxyError {
    pub fn retry_class(&self) -> RetryClass {
        match self {
            AnimaProxyError::Transport(_) => RetryClass::Retryable,
            AnimaProxyError::Substrate(s) => match s.code() {
                Code::Unavailable
                | Code::DeadlineExceeded
                | Code::Aborted
                | Code::ResourceExhausted => RetryClass::Retryable,
                _ => RetryClass::Permanent,
            },
            AnimaProxyError::InvalidResponse(_) => RetryClass::Permanent,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self.retry_class(), RetryClass::Retryable)
    }
}

impl From<tonic::Status> for AnimaProxyError {
    fn from(s: tonic::Status) -> Self {
        AnimaProxyError::Substrate(s)
    }
}

impl From<AnimaProxyError> for tonic::Status {
    fn from(e: AnimaProxyError) -> Self {
        match e {
            AnimaProxyError::Transport(m) => tonic::Status::unavailable(m),
            AnimaProxyError::Substrate(s) => s,
            AnimaProxyError::InvalidResponse(m) => tonic::Status::internal(m),
        }
    }
}

pub type AnimaProxyResult<T> = Result<T, AnimaProxyError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aborted_is_retryable() {
        let e = AnimaProxyError::Substrate(tonic::Status::aborted("retry me"));
        assert!(e.is_retryable());
    }

    #[test]
    fn unauthenticated_is_permanent() {
        let e = AnimaProxyError::Substrate(tonic::Status::unauthenticated("nope"));
        assert_eq!(e.retry_class(), RetryClass::Permanent);
    }
}
