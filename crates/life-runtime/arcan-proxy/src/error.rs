//! Error surface for the arcan-proxy crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArcanProxyError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("substrate: {0}")]
    Substrate(tonic::Status),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
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
