//! AgentSelf — the composite identity of an agent.
//!
//! AgentSelf is the union of Soul + Identity + Belief. It is the
//! single type that all other Life crates consume when they need
//! to know "who is this agent?"
//!
//! - **Soul** tells you what the agent *is* (immutable origin)
//! - **Identity** tells you how the agent *proves* it is itself (crypto)
//! - **Belief** tells you what the agent *thinks* about itself (mutable)
//!
//! Together, they form a complete sense of self.

use serde::{Deserialize, Serialize};

use crate::belief::AgentBelief;
use crate::error::{AnimaError, AnimaResult};
use crate::identity::AgentIdentity;
use crate::identity_document::{
    AgentIdentityDocument, AgentType, IdentityDocumentBuilder, VerificationMethod,
};
use crate::soul::AgentSoul;

/// The composite self of an agent — soul, identity, and beliefs.
///
/// This is the primary type consumed by all Life crates. When Arcan
/// starts its agent loop, it reconstructs AgentSelf from Lago.
/// When Autonomic evaluates regulation, it reads AgentSelf's beliefs.
/// When Haima processes a payment, it uses AgentSelf's identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSelf {
    /// The immutable soul — origin, lineage, values.
    soul: AgentSoul,

    /// The cryptographic identity — keypairs, lifecycle.
    identity: AgentIdentity,

    /// The mutable beliefs — capabilities, trust, reputation.
    beliefs: AgentBelief,
}

impl AgentSelf {
    /// Construct an AgentSelf from its components.
    ///
    /// Validates that the identity's public key matches the soul's
    /// root key, and that beliefs are consistent with the soul's policy.
    pub fn new(
        soul: AgentSoul,
        identity: AgentIdentity,
        beliefs: AgentBelief,
    ) -> AnimaResult<Self> {
        // Verify the identity's auth key matches the soul's root key
        if identity.auth_public_key != soul.root_public_key() {
            return Err(AnimaError::Identity(
                "identity auth key does not match soul's root key".into(),
            ));
        }

        // Verify beliefs are consistent with the soul's policy
        beliefs.validate_against_policy(soul.values())?;

        Ok(Self {
            soul,
            identity,
            beliefs,
        })
    }

    /// Construct without validation (for deserialization/reconstruction).
    ///
    /// Use this only when loading from a trusted source (Lago journal).
    pub fn from_parts_unchecked(
        soul: AgentSoul,
        identity: AgentIdentity,
        beliefs: AgentBelief,
    ) -> Self {
        Self {
            soul,
            identity,
            beliefs,
        }
    }

    // === Accessors ===

    /// The agent's immutable soul.
    pub fn soul(&self) -> &AgentSoul {
        &self.soul
    }

    /// The agent's cryptographic identity.
    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }

    /// The agent's mutable beliefs (read-only access).
    pub fn beliefs(&self) -> &AgentBelief {
        &self.beliefs
    }

    /// Mutable access to beliefs for updates.
    ///
    /// After mutation, call `validate()` to ensure consistency.
    pub fn beliefs_mut(&mut self) -> &mut AgentBelief {
        &mut self.beliefs
    }

    /// Mutable access to identity for lifecycle transitions.
    pub fn identity_mut(&mut self) -> &mut AgentIdentity {
        &mut self.identity
    }

    /// The agent's unique ID.
    pub fn agent_id(&self) -> &str {
        &self.identity.agent_id
    }

    /// The agent's name (from soul).
    pub fn name(&self) -> &str {
        self.soul.name()
    }

    /// The agent's mission (from soul).
    pub fn mission(&self) -> &str {
        self.soul.mission()
    }

    /// The soul hash (tamper-evident seal).
    pub fn soul_hash(&self) -> &str {
        self.soul.soul_hash()
    }

    /// The agent's DID (Decentralized Identifier), if available.
    pub fn did(&self) -> Option<&str> {
        self.identity.did.as_deref()
    }

    /// Whether the agent's identity is currently active.
    pub fn is_active(&self) -> bool {
        self.identity.is_active()
    }

    // === Validation ===

    /// Validate the entire self for consistency.
    ///
    /// Checks:
    /// 1. Soul integrity (hash matches content)
    /// 2. Identity key matches soul's root key
    /// 3. Beliefs comply with soul's PolicyManifest
    pub fn validate(&self) -> AnimaResult<()> {
        self.soul.verify_integrity()?;

        if self.identity.auth_public_key != self.soul.root_public_key() {
            return Err(AnimaError::Identity(
                "identity auth key does not match soul's root key".into(),
            ));
        }

        self.beliefs.validate_against_policy(self.soul.values())?;

        Ok(())
    }

    /// Produce a human-readable summary for audit purposes.
    pub fn audit_summary(&self) -> String {
        format!(
            "AgentSelf[id={}, name={}, active={}, capabilities={}, trust_peers={}, soul={}]",
            self.identity.agent_id,
            self.soul.name(),
            self.is_active(),
            self.beliefs.capabilities.len(),
            self.beliefs.trust_scores.len(),
            &self.soul.soul_hash()[..16],
        )
    }

    /// Generate a KYA (Know Your Agent) identity document from this AgentSelf.
    ///
    /// The document combines identity, soul, and belief data into a
    /// single verifiable identity package. Optionally enriched with
    /// an external trust score (from Autonomic's trust-score API).
    pub fn identity_document(
        &self,
        agent_type: AgentType,
        trust_score: Option<f64>,
    ) -> AnimaResult<AgentIdentityDocument> {
        let did = self.identity.did.as_ref().ok_or_else(|| {
            AnimaError::Did("agent identity has no DID — generate one first".into())
        })?;

        let auth_key_multibase = {
            let mut bytes = vec![0xed, 0x01]; // Ed25519 multicodec prefix
            bytes.extend_from_slice(&self.identity.auth_public_key);
            let encoded = bs58::encode(&bytes).into_string();
            format!("z{encoded}")
        };

        let vm = VerificationMethod {
            id: format!("{did}#key-1"),
            method_type: "Ed25519VerificationKey2020".into(),
            controller: did.clone(),
            public_key_multibase: auth_key_multibase,
        };

        let capabilities: Vec<String> = self
            .beliefs
            .capabilities
            .iter()
            .filter(|c| c.expires_at.is_none_or(|exp| chrono::Utc::now() < exp))
            .map(|c| c.capability.clone())
            .collect();

        let mut builder = IdentityDocumentBuilder::new(
            did.clone(),
            self.soul.name().to_string(),
            self.soul.mission().to_string(),
            self.soul.soul_hash().to_string(),
        )
        .agent_type(agent_type)
        .verification_method(vm)
        .capabilities(capabilities)
        .created_at(self.identity.created_at);

        if let Some(score) = trust_score {
            builder = builder.trust_score(score);
        }

        Ok(builder.build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::LifecycleState;
    use crate::soul::SoulBuilder;
    use chrono::Utc;
    use haima_core::wallet::{ChainId, WalletAddress};

    fn test_soul() -> AgentSoul {
        SoulBuilder::new("test-agent", "test mission", vec![1u8; 32]).build()
    }

    fn test_identity() -> AgentIdentity {
        AgentIdentity {
            agent_id: "agt_001".into(),
            host_id: "host_test".into(),
            auth_public_key: vec![1u8; 32], // Must match soul's root key
            wallet_address: WalletAddress {
                address: "0xtest".into(),
                chain: ChainId::base(),
            },
            did: None,
            lifecycle: LifecycleState::Active,
            created_at: Utc::now(),
            expires_at: None,
            seed_blob_ref: None,
        }
    }

    #[test]
    fn construct_valid_self() {
        let agent = AgentSelf::new(test_soul(), test_identity(), AgentBelief::default());
        assert!(agent.is_ok());
    }

    #[test]
    fn mismatched_key_fails() {
        let mut id = test_identity();
        id.auth_public_key = vec![2u8; 32]; // Different from soul's root key

        let result = AgentSelf::new(test_soul(), id, AgentBelief::default());
        assert!(result.is_err());
    }

    #[test]
    fn validate_checks_all_layers() {
        let agent = AgentSelf::new(test_soul(), test_identity(), AgentBelief::default()).unwrap();

        assert!(agent.validate().is_ok());
    }

    #[test]
    fn belief_mutation_and_validation() {
        let soul = test_soul();
        let policy = soul.values().clone();
        let mut agent = AgentSelf::new(soul, test_identity(), AgentBelief::default()).unwrap();

        // Grant a capability within the ceiling
        let grant = crate::belief::GrantedCapability {
            capability: "chat:send".into(),
            granted_by: "server".into(),
            granted_at: Utc::now(),
            expires_at: None,
            constraints: vec![],
        };

        agent
            .beliefs_mut()
            .grant_capability(grant, &policy)
            .unwrap();
        assert!(agent.validate().is_ok());
        assert!(agent.beliefs().has_capability("chat:send"));
    }
}
