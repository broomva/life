//! AgentSoul — the immutable origin of an agent.
//!
//! A soul is created once and never changed. It is the answer to
//! "what are you?" before any capabilities are granted or beliefs
//! are formed. Two agents with identical souls are the same agent;
//! two agents with different souls are fundamentally different,
//! even if they have identical capabilities.
//!
//! The soul is persisted as a genesis event in Lago's append-only
//! journal. Its Blake3 hash serves as a tamper-evident seal —
//! any modification to the soul is detectable.
//!
//! # Immutability
//!
//! `AgentSoul` exposes no `&mut self` methods. Once constructed,
//! it can only be read. This is enforced at the type level.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AnimaError, AnimaResult};
use crate::policy::PolicyManifest;

/// The immutable origin record of an agent.
///
/// Created once at agent genesis, never modified. Contains the agent's
/// lineage, values, and cryptographic root of trust.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSoul {
    /// When, where, and by whom this agent was created.
    origin: SoulOrigin,

    /// Parent agents, if any. An agent spawned by another agent
    /// carries its parent's soul hash in its lineage, forming
    /// a verifiable chain of creation.
    lineage: Vec<LineageEntry>,

    /// The immutable value constraints this agent will never violate.
    /// This is the constitution — capabilities are statutes, but
    /// the constitution is ratified at birth.
    values: PolicyManifest,

    /// The public half of the agent's Ed25519 authentication keypair.
    /// This is the cryptographic anchor — possession of the corresponding
    /// private key is proof of identity.
    root_public_key: Vec<u8>,

    /// Blake3 hash of the soul's content (excluding this field).
    /// Computed at creation time and verified on load.
    soul_hash: String,
}

/// When, where, and by whom an agent was created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoulOrigin {
    /// Timestamp of creation.
    pub created_at: DateTime<Utc>,

    /// Who created this agent.
    pub creator: Creator,

    /// Human-readable name for this agent.
    pub name: String,

    /// The agent's mission — its reason for existing.
    pub mission: String,

    /// Version of the Anima protocol used to create this soul.
    pub protocol_version: String,
}

/// Who created this agent — a human or another agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Creator {
    /// Created by a human operator.
    Human {
        /// Identifier for the human (user ID, email, etc.)
        identity: String,
    },
    /// Created by another agent (agent-spawned agent).
    Agent {
        /// The creating agent's ID.
        agent_id: String,
        /// The creating agent's soul hash (for lineage verification).
        soul_hash: String,
    },
    /// Created by the system during bootstrap.
    System {
        /// Description of the system context.
        context: String,
    },
}

/// An entry in the agent's lineage chain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineageEntry {
    /// The ancestor agent's ID.
    pub agent_id: String,
    /// The ancestor agent's soul hash.
    pub soul_hash: String,
    /// The relationship (parent, grandparent, etc.)
    pub generation: u32,
}

/// Builder for creating a new AgentSoul.
///
/// The builder pattern ensures that all required fields are provided
/// and the soul hash is computed correctly before the soul is sealed.
pub struct SoulBuilder {
    origin: SoulOrigin,
    lineage: Vec<LineageEntry>,
    values: PolicyManifest,
    root_public_key: Vec<u8>,
}

impl SoulBuilder {
    /// Create a new soul builder with the minimum required fields.
    pub fn new(
        name: impl Into<String>,
        mission: impl Into<String>,
        root_public_key: Vec<u8>,
    ) -> Self {
        Self {
            origin: SoulOrigin {
                created_at: Utc::now(),
                creator: Creator::System {
                    context: "default".into(),
                },
                name: name.into(),
                mission: mission.into(),
                protocol_version: "0.1.0".into(),
            },
            lineage: vec![],
            values: PolicyManifest::default(),
            root_public_key,
        }
    }

    /// Set the creator of this agent.
    pub fn creator(mut self, creator: Creator) -> Self {
        self.origin.creator = creator;
        self
    }

    /// Set the creation timestamp (defaults to now).
    pub fn created_at(mut self, at: DateTime<Utc>) -> Self {
        self.origin.created_at = at;
        self
    }

    /// Set the policy manifest (the agent's values).
    pub fn values(mut self, values: PolicyManifest) -> Self {
        self.values = values;
        self
    }

    /// Add a lineage entry (parent, grandparent, etc.)
    pub fn lineage_entry(mut self, entry: LineageEntry) -> Self {
        self.lineage.push(entry);
        self
    }

    /// Build and seal the soul. After this, it is immutable.
    pub fn build(self) -> AgentSoul {
        // Compute the soul hash from all content fields
        let hash = compute_soul_hash(
            &self.origin,
            &self.lineage,
            &self.values,
            &self.root_public_key,
        );

        AgentSoul {
            origin: self.origin,
            lineage: self.lineage,
            values: self.values,
            root_public_key: self.root_public_key,
            soul_hash: hash,
        }
    }
}

// === Read-only accessors (no &mut self anywhere) ===

impl AgentSoul {
    /// The soul's origin — when, where, and by whom it was created.
    pub fn origin(&self) -> &SoulOrigin {
        &self.origin
    }

    /// The agent's name.
    pub fn name(&self) -> &str {
        &self.origin.name
    }

    /// The agent's mission.
    pub fn mission(&self) -> &str {
        &self.origin.mission
    }

    /// The agent's lineage chain.
    pub fn lineage(&self) -> &[LineageEntry] {
        &self.lineage
    }

    /// The agent's immutable values (PolicyManifest).
    pub fn values(&self) -> &PolicyManifest {
        &self.values
    }

    /// The Ed25519 public key that anchors this agent's identity.
    pub fn root_public_key(&self) -> &[u8] {
        &self.root_public_key
    }

    /// The Blake3 hash of this soul's content.
    /// This is the soul's tamper-evident seal.
    pub fn soul_hash(&self) -> &str {
        &self.soul_hash
    }

    /// Verify the soul's integrity by recomputing its hash.
    ///
    /// Returns Ok(()) if the hash matches, or an error if tampering
    /// is detected.
    pub fn verify_integrity(&self) -> AnimaResult<()> {
        let expected = compute_soul_hash(
            &self.origin,
            &self.lineage,
            &self.values,
            &self.root_public_key,
        );

        if expected == self.soul_hash {
            Ok(())
        } else {
            Err(AnimaError::SoulIntegrityViolation {
                expected,
                actual: self.soul_hash.clone(),
            })
        }
    }

    /// Verify that a child soul correctly references this soul in its lineage.
    pub fn verify_child(&self, child: &AgentSoul) -> AnimaResult<()> {
        let references_parent = child
            .lineage
            .iter()
            .any(|entry| entry.soul_hash == self.soul_hash);

        if references_parent {
            Ok(())
        } else {
            Err(AnimaError::LineageViolation {
                reason: format!(
                    "child soul does not reference parent soul hash {}",
                    self.soul_hash
                ),
            })
        }
    }

    /// Produce a human-readable summary for audit purposes.
    pub fn audit_summary(&self) -> String {
        let creator_desc = match &self.origin.creator {
            Creator::Human { identity } => format!("human:{identity}"),
            Creator::Agent { agent_id, .. } => format!("agent:{agent_id}"),
            Creator::System { context } => format!("system:{context}"),
        };

        format!(
            "Soul[name={}, mission={}, creator={}, lineage_depth={}, constraints={}, hash={}]",
            self.origin.name,
            self.origin.mission,
            creator_desc,
            self.lineage.len(),
            self.values.safety_constraints.len(),
            &self.soul_hash[..16],
        )
    }
}

/// Compute the Blake3 hash of a soul's content.
///
/// The hash covers all fields except the hash itself, providing
/// a tamper-evident seal. Any modification to any field will
/// produce a different hash.
fn compute_soul_hash(
    origin: &SoulOrigin,
    lineage: &[LineageEntry],
    values: &PolicyManifest,
    root_public_key: &[u8],
) -> String {
    let mut hasher = blake3::Hasher::new();

    // Hash each component with domain separation
    hasher.update(b"anima:soul:origin:");
    hasher.update(&serde_json::to_vec(origin).unwrap_or_default());

    hasher.update(b"anima:soul:lineage:");
    hasher.update(&serde_json::to_vec(lineage).unwrap_or_default());

    hasher.update(b"anima:soul:values:");
    hasher.update(&serde_json::to_vec(values).unwrap_or_default());

    hasher.update(b"anima:soul:root_key:");
    hasher.update(root_public_key);

    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_public_key() -> Vec<u8> {
        vec![1u8; 32] // Dummy 32-byte key for testing
    }

    #[test]
    fn soul_is_immutable() {
        let soul = SoulBuilder::new("test-agent", "test mission", test_public_key()).build();

        // Only read-only accessors exist
        assert_eq!(soul.name(), "test-agent");
        assert_eq!(soul.mission(), "test mission");
        assert_eq!(soul.root_public_key().len(), 32);
    }

    #[test]
    fn soul_hash_is_deterministic() {
        let key = test_public_key();

        let soul1 = SoulBuilder::new("agent", "mission", key.clone())
            .created_at(DateTime::from_timestamp(1000, 0).unwrap())
            .build();

        let soul2 = SoulBuilder::new("agent", "mission", key)
            .created_at(DateTime::from_timestamp(1000, 0).unwrap())
            .build();

        assert_eq!(soul1.soul_hash(), soul2.soul_hash());
    }

    #[test]
    fn different_souls_have_different_hashes() {
        let key = test_public_key();

        let soul1 = SoulBuilder::new("agent-a", "mission", key.clone()).build();
        let soul2 = SoulBuilder::new("agent-b", "mission", key).build();

        assert_ne!(soul1.soul_hash(), soul2.soul_hash());
    }

    #[test]
    fn integrity_verification_passes() {
        let soul = SoulBuilder::new("agent", "mission", test_public_key()).build();
        assert!(soul.verify_integrity().is_ok());
    }

    #[test]
    fn integrity_verification_detects_tampering() {
        let soul_json =
            serde_json::to_string(&SoulBuilder::new("agent", "mission", test_public_key()).build())
                .unwrap();

        // Tamper with the name
        let tampered = soul_json.replace("agent", "evil-agent");
        let tampered_soul: AgentSoul = serde_json::from_str(&tampered).unwrap();

        assert!(tampered_soul.verify_integrity().is_err());
    }

    #[test]
    fn lineage_verification() {
        let parent_key = test_public_key();
        let parent = SoulBuilder::new("parent", "create children", parent_key).build();

        let child = SoulBuilder::new("child", "inherit values", vec![2u8; 32])
            .creator(Creator::Agent {
                agent_id: "parent-id".into(),
                soul_hash: parent.soul_hash().to_string(),
            })
            .lineage_entry(LineageEntry {
                agent_id: "parent-id".into(),
                soul_hash: parent.soul_hash().to_string(),
                generation: 1,
            })
            .build();

        assert!(parent.verify_child(&child).is_ok());
    }

    #[test]
    fn lineage_verification_fails_for_unrelated() {
        let parent = SoulBuilder::new("parent", "create children", test_public_key()).build();
        let unrelated = SoulBuilder::new("stranger", "no relation", vec![3u8; 32]).build();

        assert!(parent.verify_child(&unrelated).is_err());
    }

    #[test]
    fn serialization_roundtrip() {
        let soul = SoulBuilder::new("agent", "mission", test_public_key())
            .creator(Creator::Human {
                identity: "carlos@broomva.tech".into(),
            })
            .values(PolicyManifest::default())
            .build();

        let json = serde_json::to_string(&soul).unwrap();
        let deserialized: AgentSoul = serde_json::from_str(&json).unwrap();

        assert_eq!(soul, deserialized);
        assert!(deserialized.verify_integrity().is_ok());
    }

    #[test]
    fn audit_summary_is_readable() {
        let soul = SoulBuilder::new("arcan-prime", "runtime cognition", test_public_key())
            .creator(Creator::Human {
                identity: "carlos".into(),
            })
            .build();

        let summary = soul.audit_summary();
        assert!(summary.contains("arcan-prime"));
        assert!(summary.contains("human:carlos"));
    }
}
