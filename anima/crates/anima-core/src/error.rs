//! Error types for the Anima crate.
//!
//! Anima errors are designed to be specific enough to diagnose,
//! but not so granular that they leak implementation details.

use thiserror::Error;

/// All errors that can arise from Anima operations.
#[derive(Debug, Error)]
pub enum AnimaError {
    /// The soul has already been created for this agent.
    /// Souls are immutable — they cannot be recreated or overwritten.
    #[error("soul already exists for agent {agent_id}")]
    SoulAlreadyExists { agent_id: String },

    /// The soul was not found when expected.
    #[error("no soul found for agent {agent_id}")]
    SoulNotFound { agent_id: String },

    /// The soul's integrity hash does not match its content.
    /// This indicates tampering or corruption.
    #[error("soul integrity violation: expected {expected}, got {actual}")]
    SoulIntegrityViolation { expected: String, actual: String },

    /// A belief update would violate the soul's PolicyManifest.
    #[error("belief violates policy: {reason}")]
    PolicyViolation { reason: String },

    /// A capability exceeds the soul's capability ceiling.
    #[error("capability {capability} exceeds ceiling defined in soul")]
    CapabilityCeilingExceeded { capability: String },

    /// Identity keypair error.
    #[error("identity error: {0}")]
    Identity(String),

    /// Key derivation or cryptographic operation failed.
    #[error("crypto error: {0}")]
    Crypto(String),

    /// JWT signing or verification failed.
    #[error("jwt error: {0}")]
    Jwt(String),

    /// Key storage error (encryption, decryption, I/O).
    #[error("keystore error: {0}")]
    Keystore(String),

    /// Persistence error (Lago journal or blob store).
    #[error("persistence error: {0}")]
    Persistence(String),

    /// Serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Lineage verification failed — child does not reference parent correctly.
    #[error("lineage verification failed: {reason}")]
    LineageViolation { reason: String },
}

/// Convenience alias for Anima results.
pub type AnimaResult<T> = Result<T, AnimaError>;
