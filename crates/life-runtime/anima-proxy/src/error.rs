//! Error surface for the anima-proxy crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AnimaProxyError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("substrate: {0}")]
    Substrate(tonic::Status),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
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
