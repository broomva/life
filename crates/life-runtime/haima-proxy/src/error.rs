//! Error surface for the haima-proxy crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HaimaProxyError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("substrate: {0}")]
    Substrate(tonic::Status),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
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
