//! AgentIdentityDocument — the KYA (Know Your Agent) identity document.
//!
//! KYA is the agent-era equivalent of KYC (Know Your Customer). It provides
//! a structured, verifiable identity document that answers:
//!
//! - **Who is this agent?** (DID, controller, type)
//! - **What can it do?** (capabilities, verification methods)
//! - **How trustworthy is it?** (trust score, tier, attestations)
//!
//! The identity document is derived from `AgentSelf` and enriched with
//! trust data from Autonomic's trust-score API.
//!
//! # Relationship to W3C DID
//!
//! The `AgentIdentityDocument` borrows concepts from W3C DID Documents
//! but is not a full DID Document implementation. It is a pragmatic
//! subset tailored to the Agent OS trust model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The type of agent — how it operates and who controls it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentType {
    /// Self-directed agent with autonomous decision-making.
    Autonomous,

    /// Acts on behalf of a human controller with delegated authority.
    Delegated,

    /// Runs within a platform (like broomva.tech) under platform governance.
    Hosted,
}

impl std::fmt::Display for AgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Autonomous => write!(f, "autonomous"),
            Self::Delegated => write!(f, "delegated"),
            Self::Hosted => write!(f, "hosted"),
        }
    }
}

/// A verification method — how to verify the agent's identity cryptographically.
///
/// Follows the W3C DID Verification Method structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationMethod {
    /// Unique identifier for this key. Format: `did:key:z6Mk...#key-1`
    pub id: String,

    /// The type of verification method.
    /// Examples: "Ed25519VerificationKey2020", "EcdsaSecp256k1VerificationKey2019"
    pub method_type: String,

    /// The controller DID — who controls this key.
    pub controller: String,

    /// The public key encoded in multibase format.
    pub public_key_multibase: String,
}

/// An attestation — a claim made by one entity about another.
///
/// Attestations are the building blocks of reputation in the agent economy.
/// They can be issued by humans, other agents, or platforms.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attestation {
    /// DID of the entity making the attestation.
    pub issuer: String,

    /// What is being attested (human-readable claim).
    /// Examples: "completed-onboarding", "passed-safety-audit", "verified-identity"
    pub claim: String,

    /// Evidence supporting the attestation (reference or hash).
    /// Could be a URL, a Lago event ID, or a content hash.
    pub evidence: String,

    /// When this attestation was issued.
    pub issued_at: DateTime<Utc>,

    /// When this attestation expires (if time-bounded).
    pub expires_at: Option<DateTime<Utc>>,
}

impl Attestation {
    /// Whether this attestation is currently valid (not expired).
    pub fn is_valid(&self) -> bool {
        self.expires_at.is_none_or(|exp| Utc::now() < exp)
    }
}

/// Trust tier — qualitative trust level derived from quantitative score.
///
/// Maps to the Autonomic trust-score API's tier system.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum TrustTier {
    /// No trust history. New or unknown agent.
    Unverified,

    /// Some positive interactions, but not yet established.
    Provisional,

    /// Established positive track record.
    Trusted,

    /// Formally verified by a trusted authority.
    Certified,
}

impl TrustTier {
    /// Derive a trust tier from a numeric trust score (0.0 - 1.0).
    pub fn from_score(score: f64) -> Self {
        if score >= 0.9 {
            Self::Certified
        } else if score >= 0.7 {
            Self::Trusted
        } else if score >= 0.4 {
            Self::Provisional
        } else {
            Self::Unverified
        }
    }
}

impl std::fmt::Display for TrustTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unverified => write!(f, "unverified"),
            Self::Provisional => write!(f, "provisional"),
            Self::Trusted => write!(f, "trusted"),
            Self::Certified => write!(f, "certified"),
        }
    }
}

/// The KYA (Know Your Agent) identity document.
///
/// This is the complete identity package for an agent. It combines
/// the DID, verification methods, capabilities, trust information,
/// and attestations into a single verifiable document.
///
/// Generated from `AgentSelf` and enriched with external trust data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentIdentityDocument {
    /// The agent's DID. Format: `did:key:z6Mk...`
    pub did: String,

    /// DID of the human or entity that controls this agent.
    /// `None` for fully autonomous agents.
    pub controller_did: Option<String>,

    /// How this agent operates (autonomous, delegated, hosted).
    pub agent_type: AgentType,

    /// Cryptographic verification methods for this agent.
    pub verification_methods: Vec<VerificationMethod>,

    /// Capabilities this agent holds.
    pub capabilities: Vec<String>,

    /// Quantitative trust score (0.0 - 1.0) from Autonomic.
    pub trust_score: Option<f64>,

    /// Qualitative trust tier derived from the score.
    pub trust_tier: Option<TrustTier>,

    /// When this identity document was created.
    pub created_at: DateTime<Utc>,

    /// When this document was last updated.
    pub updated_at: DateTime<Utc>,

    /// Attestations received by this agent.
    pub attestations: Vec<Attestation>,

    /// The agent's human-readable name.
    pub name: String,

    /// The agent's mission statement.
    pub mission: String,

    /// Soul hash — tamper-evident seal of the agent's immutable origin.
    pub soul_hash: String,
}

impl AgentIdentityDocument {
    /// Whether this agent has any valid attestations.
    pub fn has_valid_attestations(&self) -> bool {
        self.attestations.iter().any(|a| a.is_valid())
    }

    /// Count of valid (non-expired) attestations.
    pub fn valid_attestation_count(&self) -> usize {
        self.attestations.iter().filter(|a| a.is_valid()).count()
    }

    /// Whether the agent has a specific capability.
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// Produce a human-readable audit summary.
    pub fn audit_summary(&self) -> String {
        format!(
            "KYA[did={}, type={}, trust={}, tier={}, caps={}, attestations={}, name={}]",
            &self.did[..self.did.len().min(32)],
            self.agent_type,
            self.trust_score
                .map(|s| format!("{s:.2}"))
                .unwrap_or_else(|| "n/a".into()),
            self.trust_tier
                .map(|t| t.to_string())
                .unwrap_or_else(|| "n/a".into()),
            self.capabilities.len(),
            self.valid_attestation_count(),
            self.name,
        )
    }
}

/// Builder for constructing an `AgentIdentityDocument` from `AgentSelf` components.
pub struct IdentityDocumentBuilder {
    did: String,
    controller_did: Option<String>,
    agent_type: AgentType,
    verification_methods: Vec<VerificationMethod>,
    capabilities: Vec<String>,
    trust_score: Option<f64>,
    trust_tier: Option<TrustTier>,
    attestations: Vec<Attestation>,
    name: String,
    mission: String,
    soul_hash: String,
    created_at: DateTime<Utc>,
}

impl IdentityDocumentBuilder {
    /// Create a new builder with the minimum required fields.
    pub fn new(did: String, name: String, mission: String, soul_hash: String) -> Self {
        Self {
            did,
            controller_did: None,
            agent_type: AgentType::Hosted,
            verification_methods: vec![],
            capabilities: vec![],
            trust_score: None,
            trust_tier: None,
            attestations: vec![],
            name,
            mission,
            soul_hash,
            created_at: Utc::now(),
        }
    }

    /// Set the controller DID.
    pub fn controller_did(mut self, controller: String) -> Self {
        self.controller_did = Some(controller);
        self
    }

    /// Set the agent type.
    pub fn agent_type(mut self, agent_type: AgentType) -> Self {
        self.agent_type = agent_type;
        self
    }

    /// Add a verification method.
    pub fn verification_method(mut self, vm: VerificationMethod) -> Self {
        self.verification_methods.push(vm);
        self
    }

    /// Set capabilities from belief.
    pub fn capabilities(mut self, caps: Vec<String>) -> Self {
        self.capabilities = caps;
        self
    }

    /// Set trust score and derive tier.
    pub fn trust_score(mut self, score: f64) -> Self {
        self.trust_score = Some(score);
        self.trust_tier = Some(TrustTier::from_score(score));
        self
    }

    /// Add an attestation.
    pub fn attestation(mut self, attestation: Attestation) -> Self {
        self.attestations.push(attestation);
        self
    }

    /// Set the creation timestamp.
    pub fn created_at(mut self, at: DateTime<Utc>) -> Self {
        self.created_at = at;
        self
    }

    /// Build the identity document.
    pub fn build(self) -> AgentIdentityDocument {
        let now = Utc::now();
        AgentIdentityDocument {
            did: self.did,
            controller_did: self.controller_did,
            agent_type: self.agent_type,
            verification_methods: self.verification_methods,
            capabilities: self.capabilities,
            trust_score: self.trust_score,
            trust_tier: self.trust_tier,
            created_at: self.created_at,
            updated_at: now,
            attestations: self.attestations,
            name: self.name,
            mission: self.mission,
            soul_hash: self.soul_hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_document() -> AgentIdentityDocument {
        IdentityDocumentBuilder::new(
            "did:key:z6MkTest".into(),
            "test-agent".into(),
            "test mission".into(),
            "hash123".into(),
        )
        .agent_type(AgentType::Autonomous)
        .capabilities(vec!["chat:send".into(), "knowledge:read".into()])
        .trust_score(0.85)
        .verification_method(VerificationMethod {
            id: "did:key:z6MkTest#key-1".into(),
            method_type: "Ed25519VerificationKey2020".into(),
            controller: "did:key:z6MkTest".into(),
            public_key_multibase: "z6MkTest".into(),
        })
        .build()
    }

    #[test]
    fn identity_document_creation() {
        let doc = test_document();
        assert_eq!(doc.did, "did:key:z6MkTest");
        assert_eq!(doc.agent_type, AgentType::Autonomous);
        assert_eq!(doc.capabilities.len(), 2);
        assert!(doc.has_capability("chat:send"));
        assert!(!doc.has_capability("admin:delete"));
    }

    #[test]
    fn trust_tier_from_score() {
        assert_eq!(TrustTier::from_score(0.0), TrustTier::Unverified);
        assert_eq!(TrustTier::from_score(0.3), TrustTier::Unverified);
        assert_eq!(TrustTier::from_score(0.4), TrustTier::Provisional);
        assert_eq!(TrustTier::from_score(0.6), TrustTier::Provisional);
        assert_eq!(TrustTier::from_score(0.7), TrustTier::Trusted);
        assert_eq!(TrustTier::from_score(0.8), TrustTier::Trusted);
        assert_eq!(TrustTier::from_score(0.9), TrustTier::Certified);
        assert_eq!(TrustTier::from_score(1.0), TrustTier::Certified);
    }

    #[test]
    fn trust_score_derives_tier() {
        let doc = test_document();
        assert_eq!(doc.trust_score, Some(0.85));
        assert_eq!(doc.trust_tier, Some(TrustTier::Trusted));
    }

    #[test]
    fn attestation_validity() {
        let valid = Attestation {
            issuer: "did:key:z6MkIssuer".into(),
            claim: "safety-audit-passed".into(),
            evidence: "lago:event:123".into(),
            issued_at: Utc::now(),
            expires_at: None,
        };
        assert!(valid.is_valid());

        let expired = Attestation {
            issuer: "did:key:z6MkIssuer".into(),
            claim: "temporary-access".into(),
            evidence: "lago:event:456".into(),
            issued_at: Utc::now() - chrono::Duration::days(30),
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
        };
        assert!(!expired.is_valid());
    }

    #[test]
    fn document_with_attestations() {
        let doc = IdentityDocumentBuilder::new(
            "did:key:z6MkTest".into(),
            "agent".into(),
            "mission".into(),
            "hash".into(),
        )
        .attestation(Attestation {
            issuer: "did:key:z6MkIssuer".into(),
            claim: "verified".into(),
            evidence: "proof".into(),
            issued_at: Utc::now(),
            expires_at: None,
        })
        .attestation(Attestation {
            issuer: "did:key:z6MkIssuer2".into(),
            claim: "expired-claim".into(),
            evidence: "proof2".into(),
            issued_at: Utc::now() - chrono::Duration::days(30),
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
        })
        .build();

        assert!(doc.has_valid_attestations());
        assert_eq!(doc.valid_attestation_count(), 1);
    }

    #[test]
    fn serialization_roundtrip() {
        let doc = test_document();
        let json = serde_json::to_string(&doc).unwrap();
        let deserialized: AgentIdentityDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, deserialized);
    }

    #[test]
    fn agent_type_display() {
        assert_eq!(AgentType::Autonomous.to_string(), "autonomous");
        assert_eq!(AgentType::Delegated.to_string(), "delegated");
        assert_eq!(AgentType::Hosted.to_string(), "hosted");
    }

    #[test]
    fn trust_tier_display() {
        assert_eq!(TrustTier::Unverified.to_string(), "unverified");
        assert_eq!(TrustTier::Provisional.to_string(), "provisional");
        assert_eq!(TrustTier::Trusted.to_string(), "trusted");
        assert_eq!(TrustTier::Certified.to_string(), "certified");
    }

    #[test]
    fn audit_summary_contains_key_fields() {
        let doc = test_document();
        let summary = doc.audit_summary();
        assert!(summary.contains("did:key:z6MkTest"));
        assert!(summary.contains("autonomous"));
        assert!(summary.contains("0.85"));
        assert!(summary.contains("trusted"));
        assert!(summary.contains("test-agent"));
    }

    #[test]
    fn builder_with_controller() {
        let doc = IdentityDocumentBuilder::new(
            "did:key:z6MkAgent".into(),
            "agent".into(),
            "mission".into(),
            "hash".into(),
        )
        .controller_did("did:key:z6MkHuman".into())
        .agent_type(AgentType::Delegated)
        .build();

        assert_eq!(doc.controller_did, Some("did:key:z6MkHuman".into()));
        assert_eq!(doc.agent_type, AgentType::Delegated);
    }

    #[test]
    fn trust_tier_ordering() {
        assert!(TrustTier::Unverified < TrustTier::Provisional);
        assert!(TrustTier::Provisional < TrustTier::Trusted);
        assert!(TrustTier::Trusted < TrustTier::Certified);
    }
}
