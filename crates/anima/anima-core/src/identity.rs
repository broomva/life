//! AgentIdentity — the cryptographic proof of "I am me."
//!
//! Every agent has a dual-keypair identity:
//!
//! - **Ed25519** for authentication (Agent Auth Protocol, JWT signing)
//! - **secp256k1** for economics (Haima payments, on-chain DID)
//!
//! Both keys are derived from a single seed via HKDF, so a single
//! secret backs the entire identity. The seed is encrypted at rest
//! using ChaCha20-Poly1305 (consistent with Haima's wallet).
//!
//! The identity also generates a DID (Decentralized Identifier) from
//! the Ed25519 public key using the `did:key` method.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use haima_core::wallet::WalletAddress;

/// The lifecycle state of an agent's identity.
///
/// Maps to Agent Auth Protocol lifecycle states.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LifecycleState {
    /// Identity created but not yet approved by a human.
    Pending,
    /// Identity is active and can authenticate.
    Active,
    /// Identity has expired (time-based).
    Expired,
    /// Identity was explicitly revoked.
    Revoked,
    /// Identity was created autonomously and not yet claimed by a human.
    Unclaimed,
}

/// The cryptographic identity of an agent.
///
/// This type holds the public components of the identity. Private keys
/// are managed by `anima-identity` and never stored in this struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentIdentity {
    /// Unique identifier for this agent.
    pub agent_id: String,

    /// The host environment this agent runs in (Claude Code, Arcan, etc.)
    pub host_id: String,

    /// Ed25519 public key bytes (32 bytes).
    /// Used for Agent Auth Protocol authentication and JWT signing.
    pub auth_public_key: Vec<u8>,

    /// secp256k1 wallet address (EVM-compatible).
    /// Used for Haima payments and on-chain identity.
    pub wallet_address: WalletAddress,

    /// DID (Decentralized Identifier) derived from the Ed25519 key.
    /// Format: `did:key:z6Mk...`
    pub did: Option<String>,

    /// Current lifecycle state.
    pub lifecycle: LifecycleState,

    /// When this identity was created.
    pub created_at: DateTime<Utc>,

    /// When this identity expires (if time-bounded).
    pub expires_at: Option<DateTime<Utc>>,

    /// Reference to the encrypted seed in Lago blob store.
    /// Format: SHA-256 hex hash of the encrypted blob.
    pub seed_blob_ref: Option<String>,
}

impl AgentIdentity {
    /// Whether this identity can currently authenticate.
    pub fn is_active(&self) -> bool {
        if self.lifecycle != LifecycleState::Active {
            return false;
        }

        if let Some(expires_at) = self.expires_at {
            Utc::now() < expires_at
        } else {
            true
        }
    }

    /// Transition to a new lifecycle state.
    pub fn transition(&mut self, new_state: LifecycleState) {
        self.lifecycle = new_state;
    }

    /// Revoke this identity. Irreversible.
    pub fn revoke(&mut self) {
        self.lifecycle = LifecycleState::Revoked;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haima_core::wallet::ChainId;

    fn test_identity() -> AgentIdentity {
        AgentIdentity {
            agent_id: "agt_test_001".into(),
            host_id: "host_arcan".into(),
            auth_public_key: vec![1u8; 32],
            wallet_address: WalletAddress {
                address: "0x1234567890abcdef1234567890abcdef12345678".into(),
                chain: ChainId::base(),
            },
            did: Some("did:key:z6MkTest".into()),
            lifecycle: LifecycleState::Active,
            created_at: Utc::now(),
            expires_at: None,
            seed_blob_ref: None,
        }
    }

    #[test]
    fn active_identity_can_authenticate() {
        let id = test_identity();
        assert!(id.is_active());
    }

    #[test]
    fn revoked_identity_cannot_authenticate() {
        let mut id = test_identity();
        id.revoke();
        assert!(!id.is_active());
        assert_eq!(id.lifecycle, LifecycleState::Revoked);
    }

    #[test]
    fn expired_identity_cannot_authenticate() {
        let mut id = test_identity();
        id.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        assert!(!id.is_active());
    }

    #[test]
    fn pending_identity_cannot_authenticate() {
        let mut id = test_identity();
        id.lifecycle = LifecycleState::Pending;
        assert!(!id.is_active());
    }
}
