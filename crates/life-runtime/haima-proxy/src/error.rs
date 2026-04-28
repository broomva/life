//! Error surface for the haima-proxy crate.
//!
//! Sub-phase D adds [`RetryClass`] discrimination via [`HaimaProxyError::retry_class`]
//! and the convenience [`HaimaProxyError::is_retryable`].

use thiserror::Error;
use tonic::Code;

#[derive(Debug, Error)]
pub enum HaimaProxyError {
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

impl HaimaProxyError {
    pub fn retry_class(&self) -> RetryClass {
        match self {
            HaimaProxyError::Transport(_) => RetryClass::Retryable,
            HaimaProxyError::Substrate(s) => match s.code() {
                Code::Unavailable
                | Code::DeadlineExceeded
                | Code::Aborted
                | Code::ResourceExhausted => RetryClass::Retryable,
                _ => RetryClass::Permanent,
            },
            HaimaProxyError::InvalidResponse(_) => RetryClass::Permanent,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self.retry_class(), RetryClass::Retryable)
    }
}

impl From<tonic::Status> for HaimaProxyError {
    fn from(s: tonic::Status) -> Self {
        HaimaProxyError::Substrate(s)
    }
}

impl From<HaimaProxyError> for tonic::Status {
    fn from(e: HaimaProxyError) -> Self {
        match e {
            HaimaProxyError::Transport(m) => tonic::Status::unavailable(m),
            HaimaProxyError::Substrate(s) => s,
            HaimaProxyError::InvalidResponse(m) => tonic::Status::internal(m),
        }
    }
}

pub type HaimaProxyResult<T> = Result<T, HaimaProxyError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_is_retryable() {
        let e = HaimaProxyError::Substrate(tonic::Status::unavailable("network"));
        assert!(e.is_retryable());
    }

    #[test]
    fn already_exists_is_permanent() {
        let e = HaimaProxyError::Substrate(tonic::Status::already_exists("dup"));
        assert_eq!(e.retry_class(), RetryClass::Permanent);
    }
}
