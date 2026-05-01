//! Anima event types for Lago journal persistence.
//!
//! All Anima events use `EventKind::Custom` with the namespace prefix
//! `"anima."`, following the same pattern as Haima ("finance.") and
//! Autonomic ("autonomic.").
//!
//! Events are the source of truth. AgentBelief is a deterministic
//! projection (fold) over the event stream.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Custody backend identifier — used in `anima.custody_migrated` events.
///
/// Spec D §"Event additions" — `BackendKind` (the enum behind the same
/// name on the `AnimaCustody` trait). `#[non_exhaustive]` so adding new
/// backends in D-Sub-B…F doesn't break match exhaustiveness on
/// downstream verifiers.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    InProcess,
    Vault,
    WebCrypto,
    Tpm,
    Soma,
    HardwareWallet,
    Remote,
}

/// All event types that Anima emits to the Lago journal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnimaEventKind {
    /// The genesis event — a soul is created. This is always the
    /// first event in an agent's journal and can never be repeated.
    SoulGenesis {
        /// The full serialized AgentSoul.
        soul: serde_json::Value,
        /// The soul's Blake3 hash.
        soul_hash: String,
    },

    /// A cryptographic identity was created or rotated.
    IdentityCreated {
        agent_id: String,
        host_id: String,
        /// Ed25519 public key (hex-encoded).
        auth_public_key_hex: String,
        /// Wallet address (EVM).
        wallet_address: String,
        /// DID document (if generated).
        did: Option<String>,
        /// Reference to encrypted seed in blob store.
        seed_blob_ref: Option<String>,
    },

    /// Identity lifecycle transition.
    IdentityTransitioned {
        agent_id: String,
        from: String,
        to: String,
        reason: String,
    },

    /// A capability was granted to the agent.
    CapabilityGranted {
        capability: String,
        granted_by: String,
        expires_at: Option<DateTime<Utc>>,
        constraints: serde_json::Value,
    },

    /// A capability was revoked from the agent.
    CapabilityRevoked {
        capability: String,
        revoked_by: String,
        reason: String,
    },

    /// Trust score updated for a peer.
    TrustUpdated {
        peer_id: String,
        new_score: f64,
        interaction_success: bool,
    },

    /// Economic belief updated (from Haima/Autonomic events).
    EconomicBeliefUpdated {
        balance_micro_credits: i64,
        burn_rate_per_hour: f64,
        economic_mode: String,
    },

    /// A belief snapshot was persisted (periodic checkpoint).
    BeliefSnapshot {
        /// The full serialized AgentBelief.
        belief: serde_json::Value,
        /// Blake3 hash of the snapshot.
        snapshot_hash: String,
    },

    /// An identity key was rotated.
    KeyRotated {
        agent_id: String,
        /// New Ed25519 public key (hex-encoded).
        new_auth_public_key_hex: String,
        /// New wallet address.
        new_wallet_address: Option<String>,
        /// Reference to new encrypted seed in blob store.
        new_seed_blob_ref: Option<String>,
        reason: String,
    },

    /// A policy violation was detected and prevented.
    PolicyViolationDetected {
        capability: String,
        reason: String,
        /// Whether the violation was blocked or just logged.
        blocked: bool,
    },

    /// Lineage verification was performed.
    LineageVerified {
        parent_soul_hash: String,
        child_soul_hash: String,
        verified: bool,
    },

    /// An attestation was received by this agent (KYA).
    IdentityAttested {
        /// DID of the attestation issuer.
        issuer_did: String,
        /// What is being attested.
        claim: String,
        /// Evidence reference.
        evidence: String,
        /// When the attestation expires (if time-bounded).
        expires_at: Option<DateTime<Utc>>,
    },

    /// Identity was verified by an external party (KYA).
    IdentityVerified {
        /// DID of the verifier.
        verifier_did: String,
        /// The verification method used.
        method: String,
        /// Whether verification succeeded.
        verified: bool,
    },

    /// Spec D §"Event additions" — the agent's auth identity rotated.
    ///
    /// Verifiers use this to walk the rotation chain: any signature by
    /// `old_did` for a seq < `rotated_at_seq` (recorded at the lago
    /// envelope level) is considered valid; any signature by `old_did`
    /// for a later seq is rejected.
    IdentityRotated {
        /// The DID that was rotated away from.
        old_did: String,
        /// The DID that signing now flows through.
        new_did: String,
        /// JWS signed by the *old* key over the *new* DID, proving the
        /// rotation was authorised.
        rotation_proof_jws: String,
        /// Wall-clock rotation timestamp (`Lago` envelope already carries
        /// the seq + microsecond timestamp — this is the operator-facing
        /// human-readable wall clock).
        rotated_at: DateTime<Utc>,
    },

    /// Spec D §"Event additions" — custody backend migrated.
    ///
    /// Documents that custody moved between backends (e.g. user upgraded
    /// from `InProcessAnima` to `TpmAnima`). Doesn't change the keys —
    /// just records the move so verifiers can audit the lineage and so
    /// operators can spot e.g. a downgrade from `Vault` back to
    /// `InProcess` (which would be a regression).
    CustodyMigrated {
        from_backend: BackendKind,
        to_backend: BackendKind,
        /// Backend-specific attestation (optional; e.g. a Vault token
        /// receipt or a TPM attestation quote).
        attestation: Option<String>,
        migrated_at: DateTime<Utc>,
    },

    /// Spec D §"Event additions" — identity revoked.
    ///
    /// After this event, no signature signed by the revoked DID is
    /// accepted regardless of seq. Used for e.g. a stolen device or a
    /// compromised passkey.
    IdentityRevoked {
        did: String,
        reason: String,
        revoked_at: DateTime<Utc>,
    },
}

impl AnimaEventKind {
    /// The event namespace prefix for Lago journal.
    pub const NAMESPACE: &'static str = "anima";

    /// Convert to the event type string for `EventKind::Custom`.
    ///
    /// Examples: "anima.soul_genesis", "anima.capability_granted"
    pub fn event_type(&self) -> String {
        let variant = match self {
            Self::SoulGenesis { .. } => "soul_genesis",
            Self::IdentityCreated { .. } => "identity_created",
            Self::IdentityTransitioned { .. } => "identity_transitioned",
            Self::CapabilityGranted { .. } => "capability_granted",
            Self::CapabilityRevoked { .. } => "capability_revoked",
            Self::TrustUpdated { .. } => "trust_updated",
            Self::EconomicBeliefUpdated { .. } => "economic_belief_updated",
            Self::BeliefSnapshot { .. } => "belief_snapshot",
            Self::KeyRotated { .. } => "key_rotated",
            Self::PolicyViolationDetected { .. } => "policy_violation_detected",
            Self::LineageVerified { .. } => "lineage_verified",
            Self::IdentityAttested { .. } => "identity_attested",
            Self::IdentityVerified { .. } => "identity_verified",
            // Spec D §"Event additions"
            Self::IdentityRotated { .. } => "identity_rotated",
            Self::CustodyMigrated { .. } => "custody_migrated",
            Self::IdentityRevoked { .. } => "identity_revoked",
        };

        format!("{}.{}", Self::NAMESPACE, variant)
    }

    /// Serialize this event to a JSON Value for use in `EventKind::Custom`.
    pub fn to_custom_data(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// Try to parse an Anima event from a custom event type + data.
    pub fn from_custom(event_type: &str, data: &serde_json::Value) -> Option<Self> {
        if !event_type.starts_with(Self::NAMESPACE) {
            return None;
        }

        serde_json::from_value(data.clone()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_formatting() {
        let event = AnimaEventKind::SoulGenesis {
            soul: serde_json::json!({}),
            soul_hash: "abc123".into(),
        };
        assert_eq!(event.event_type(), "anima.soul_genesis");

        let event = AnimaEventKind::CapabilityGranted {
            capability: "chat:send".into(),
            granted_by: "server".into(),
            expires_at: None,
            constraints: serde_json::json!({}),
        };
        assert_eq!(event.event_type(), "anima.capability_granted");
    }

    #[test]
    fn roundtrip_through_custom() {
        let original = AnimaEventKind::TrustUpdated {
            peer_id: "peer-1".into(),
            new_score: 0.85,
            interaction_success: true,
        };

        let event_type = original.event_type();
        let data = original.to_custom_data();

        let parsed = AnimaEventKind::from_custom(&event_type, &data);
        assert_eq!(parsed, Some(original));
    }

    #[test]
    fn non_anima_events_return_none() {
        let data = serde_json::json!({"amount": 100});
        assert!(AnimaEventKind::from_custom("finance.payment_settled", &data).is_none());
    }

    #[test]
    fn identity_rotated_event_type() {
        let event = AnimaEventKind::IdentityRotated {
            old_did: "did:key:z6MkOld".into(),
            new_did: "did:key:zDnNew".into(),
            rotation_proof_jws: "h.b.s".into(),
            rotated_at: chrono::Utc::now(),
        };
        assert_eq!(event.event_type(), "anima.identity_rotated");

        let data = event.to_custom_data();
        let parsed = AnimaEventKind::from_custom("anima.identity_rotated", &data);
        assert_eq!(parsed, Some(event));
    }

    #[test]
    fn custody_migrated_event_type() {
        let event = AnimaEventKind::CustodyMigrated {
            from_backend: BackendKind::InProcess,
            to_backend: BackendKind::Vault,
            attestation: Some("vault-token-123".into()),
            migrated_at: chrono::Utc::now(),
        };
        assert_eq!(event.event_type(), "anima.custody_migrated");

        let data = event.to_custom_data();
        let parsed = AnimaEventKind::from_custom("anima.custody_migrated", &data);
        assert_eq!(parsed, Some(event));
    }

    #[test]
    fn identity_revoked_event_type() {
        let event = AnimaEventKind::IdentityRevoked {
            did: "did:key:zDnRevoked".into(),
            reason: "device lost".into(),
            revoked_at: chrono::Utc::now(),
        };
        assert_eq!(event.event_type(), "anima.identity_revoked");

        let data = event.to_custom_data();
        let parsed = AnimaEventKind::from_custom("anima.identity_revoked", &data);
        assert_eq!(parsed, Some(event));
    }

    #[test]
    fn backend_kind_serialises_snake_case() {
        let json = serde_json::to_string(&BackendKind::InProcess).unwrap();
        assert_eq!(json, "\"in_process\"");
        let json = serde_json::to_string(&BackendKind::HardwareWallet).unwrap();
        assert_eq!(json, "\"hardware_wallet\"");
    }
}
