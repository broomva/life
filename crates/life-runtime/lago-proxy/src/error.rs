//! Error surface for the lago-proxy crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LagoProxyError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("substrate: {0}")]
    Substrate(tonic::Status),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
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
