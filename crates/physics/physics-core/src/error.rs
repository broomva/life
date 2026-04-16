//! Error types for the physics engine.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PhysicsError {
    #[error("invalid body handle: {0}")]
    InvalidBodyHandle(u32),

    #[error("invalid shape: {reason}")]
    InvalidShape { reason: String },

    #[error("invalid parameter: {reason}")]
    InvalidParameter { reason: String },
}

pub type PhysicsResult<T> = Result<T, PhysicsError>;
