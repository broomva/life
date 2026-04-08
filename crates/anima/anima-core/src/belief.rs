//! AgentBelief — the mutable self-model of an agent.
//!
//! Unlike the Soul (immutable), beliefs change over time as the agent
//! gains and loses capabilities, interacts with other agents, transacts
//! economically, and receives feedback from Autonomic regulation.
//!
//! Beliefs are **constrained by the Soul's PolicyManifest** — a belief
//! update that would violate the soul's values is rejected at write time.
//! This is the enforcement mechanism: the soul defines the constitution,
//! and belief updates are the legislation that must comply.
//!
//! Beliefs are projected from events (like Haima's FinancialState and
//! Autonomic's HomeostaticState). They are a pure fold over the event
//! stream, making them deterministic and reproducible.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AnimaError, AnimaResult};
use crate::policy::PolicyManifest;

/// A detected knowledge gap — a topic the agent needs but does not yet have.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeGap {
    /// The topic slug (e.g., "rust-async-patterns", "x402-protocol").
    pub topic: String,

    /// When this gap was first identified.
    pub identified_at: DateTime<Utc>,

    /// Priority of filling this gap (0.0 = low, 1.0 = critical).
    pub priority: f64,

    /// What triggered the gap detection (e.g., "tool_call:knowledge:read", "task_failure").
    pub source: String,
}

/// The agent's mutable model of its own capabilities, constraints,
/// trust relationships, and economic situation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentBelief {
    /// What the agent can currently do (from Agent Auth grants).
    pub capabilities: Vec<GrantedCapability>,

    /// Active constraints on the agent (from grants + Autonomic regulation).
    pub constraints: Vec<ActiveConstraint>,

    /// Trust in other agents and services.
    pub trust_scores: HashMap<String, TrustScore>,

    /// The agent's model of how others perceive it.
    pub reputation: ReputationVector,

    /// The agent's understanding of its economic situation.
    pub economic_belief: EconomicBelief,

    /// Knowledge relevance scores — which topics matter to this agent's mission.
    /// Projected from `knowledge.accessed` events. Keys are topic slugs.
    #[serde(default)]
    pub knowledge_relevance: HashMap<String, f64>,

    /// Known knowledge gaps — topics the agent needs but doesn't have.
    #[serde(default)]
    pub knowledge_gaps: Vec<KnowledgeGap>,

    /// Timestamp of the last belief update.
    pub last_updated: DateTime<Utc>,

    /// Sequence number of the last event that updated this belief.
    pub last_event_seq: u64,
}

/// A capability that has been granted to this agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GrantedCapability {
    /// The capability string (e.g., "chat:send", "knowledge:read").
    pub capability: String,

    /// Who granted this capability.
    pub granted_by: String,

    /// When this grant was made.
    pub granted_at: DateTime<Utc>,

    /// When this grant expires (if time-bounded).
    pub expires_at: Option<DateTime<Utc>>,

    /// Constraints on this specific grant.
    pub constraints: Vec<CapabilityConstraint>,
}

/// A constraint on a capability grant (from Agent Auth Protocol).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityConstraint {
    /// The field being constrained (e.g., "amount", "frequency").
    pub field: String,

    /// The constraint operator and value.
    pub constraint: ConstraintValue,
}

/// Constraint operators for capability grants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ConstraintValue {
    Maximum { value: serde_json::Value },
    Minimum { value: serde_json::Value },
    OneOf { values: Vec<serde_json::Value> },
    NoneOf { values: Vec<serde_json::Value> },
}

/// An active constraint imposed on the agent (by Autonomic or policy).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveConstraint {
    /// Machine-readable ID for this constraint.
    pub id: String,

    /// Source of this constraint.
    pub source: ConstraintSource,

    /// Human-readable description.
    pub description: String,

    /// When this constraint was imposed.
    pub imposed_at: DateTime<Utc>,
}

/// Where a constraint came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintSource {
    /// From the soul's PolicyManifest (permanent).
    Soul,
    /// From Autonomic regulation (dynamic).
    Autonomic,
    /// From an Agent Auth server (grant-scoped).
    AgentAuth,
    /// From an operator (administrative).
    Operator,
}

/// Trust score for another agent or service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustScore {
    /// Trust level (0.0 = no trust, 1.0 = full trust).
    pub score: f64,

    /// Number of successful interactions.
    pub successful_interactions: u64,

    /// Number of failed interactions.
    pub failed_interactions: u64,

    /// When this trust was last updated.
    pub last_interaction: DateTime<Utc>,
}

impl TrustScore {
    /// Apply time-based decay to the trust score.
    ///
    /// Trust decays toward 0.5 (neutral) over time without reinforcement.
    /// The decay rate is proportional to the time since last interaction.
    pub fn decay(&mut self, now: DateTime<Utc>) {
        let elapsed = now.signed_duration_since(self.last_interaction);
        let days = elapsed.num_hours() as f64 / 24.0;

        if days > 0.0 {
            // Exponential decay toward 0.5 (neutral) with half-life of 30 days
            let decay_factor = 0.5_f64.powf(days / 30.0);
            self.score = 0.5 + (self.score - 0.5) * decay_factor;
        }
    }
}

/// The agent's model of how others perceive it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ReputationVector {
    /// Overall reputation score (0.0 = bad, 1.0 = excellent).
    pub overall: f64,

    /// Number of successful task completions visible to others.
    pub tasks_completed: u64,

    /// Number of task failures visible to others.
    pub tasks_failed: u64,

    /// Number of policy violations recorded.
    pub violations: u64,
}

/// The agent's understanding of its economic situation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EconomicBelief {
    /// Current balance in micro-credits (projected from Haima events).
    pub balance_micro_credits: i64,

    /// Estimated burn rate (micro-credits per hour).
    pub burn_rate_per_hour: f64,

    /// Time until funds are exhausted at current burn rate.
    pub hours_until_exhaustion: Option<f64>,

    /// Current economic mode (from Autonomic).
    pub economic_mode: String,

    /// Session spend so far.
    pub session_spend_micro_credits: i64,
}

impl Default for EconomicBelief {
    fn default() -> Self {
        Self {
            balance_micro_credits: 0,
            burn_rate_per_hour: 0.0,
            hours_until_exhaustion: None,
            economic_mode: "sovereign".into(),
            session_spend_micro_credits: 0,
        }
    }
}

impl Default for AgentBelief {
    fn default() -> Self {
        Self {
            capabilities: vec![],
            constraints: vec![],
            trust_scores: HashMap::new(),
            reputation: ReputationVector::default(),
            economic_belief: EconomicBelief::default(),
            knowledge_relevance: HashMap::new(),
            knowledge_gaps: Vec::new(),
            last_updated: Utc::now(),
            last_event_seq: 0,
        }
    }
}

impl AgentBelief {
    /// Add a capability grant, enforcing the soul's PolicyManifest.
    ///
    /// Returns an error if the capability exceeds the soul's ceiling.
    pub fn grant_capability(
        &mut self,
        grant: GrantedCapability,
        policy: &PolicyManifest,
    ) -> AnimaResult<()> {
        if !policy.allows_capability(&grant.capability) {
            return Err(AnimaError::CapabilityCeilingExceeded {
                capability: grant.capability,
            });
        }

        self.capabilities.push(grant);
        self.last_updated = Utc::now();
        Ok(())
    }

    /// Revoke a capability by name.
    pub fn revoke_capability(&mut self, capability: &str) {
        self.capabilities.retain(|c| c.capability != capability);
        self.last_updated = Utc::now();
    }

    /// Check whether the agent currently believes it has a capability.
    pub fn has_capability(&self, capability: &str) -> bool {
        let now = Utc::now();
        self.capabilities
            .iter()
            .any(|c| c.capability == capability && c.expires_at.is_none_or(|exp| now < exp))
    }

    /// Update trust for a peer based on interaction outcome.
    pub fn update_trust(&mut self, peer_id: &str, success: bool) {
        let now = Utc::now();
        let entry = self
            .trust_scores
            .entry(peer_id.to_string())
            .or_insert(TrustScore {
                score: 0.5, // Start neutral
                successful_interactions: 0,
                failed_interactions: 0,
                last_interaction: now,
            });

        if success {
            entry.successful_interactions += 1;
            // Increase trust, bounded at 1.0
            entry.score = (entry.score + 0.05).min(1.0);
        } else {
            entry.failed_interactions += 1;
            // Decrease trust, bounded at 0.0
            entry.score = (entry.score - 0.1).max(0.0);
        }

        entry.last_interaction = now;
        self.last_updated = now;
    }

    /// Apply time-based trust decay to all peers.
    pub fn decay_all_trust(&mut self) {
        let now = Utc::now();
        for score in self.trust_scores.values_mut() {
            score.decay(now);
        }
    }

    /// Record that a knowledge topic was accessed, boosting its relevance.
    ///
    /// The relevance score is clamped to `[0.0, 1.0]`. Repeated access
    /// accumulates relevance, modelling the agent's growing expertise.
    pub fn record_knowledge_access(&mut self, topic: &str, boost: f64) {
        let entry = self
            .knowledge_relevance
            .entry(topic.to_string())
            .or_insert(0.0);
        *entry = (*entry + boost).min(1.0);
        self.last_updated = Utc::now();
    }

    /// Record a knowledge gap — something the agent needs but lacks.
    ///
    /// Duplicate topics are silently ignored (idempotent).
    pub fn record_knowledge_gap(&mut self, topic: String, priority: f64, source: String) {
        if !self.knowledge_gaps.iter().any(|g| g.topic == topic) {
            self.knowledge_gaps.push(KnowledgeGap {
                topic,
                identified_at: Utc::now(),
                priority,
                source,
            });
            self.last_updated = Utc::now();
        }
    }

    /// Remove a gap when the knowledge is acquired.
    pub fn resolve_knowledge_gap(&mut self, topic: &str) {
        let before = self.knowledge_gaps.len();
        self.knowledge_gaps.retain(|g| g.topic != topic);
        if self.knowledge_gaps.len() != before {
            self.last_updated = Utc::now();
        }
    }

    /// Get top-k most relevant knowledge topics, sorted by descending relevance.
    pub fn top_knowledge_topics(&self, k: usize) -> Vec<(&str, f64)> {
        let mut sorted: Vec<_> = self
            .knowledge_relevance
            .iter()
            .map(|(t, s)| (t.as_str(), *s))
            .collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(k);
        sorted
    }

    /// Validate that the entire belief state is consistent with the soul's policy.
    pub fn validate_against_policy(&self, policy: &PolicyManifest) -> AnimaResult<()> {
        for cap in &self.capabilities {
            if !policy.allows_capability(&cap.capability) {
                return Err(AnimaError::PolicyViolation {
                    reason: format!("capability '{}' exceeds soul's ceiling", cap.capability),
                });
            }
        }

        if self.economic_belief.session_spend_micro_credits
            > policy.economic_limits.max_spend_per_session_micro_credits
        {
            return Err(AnimaError::PolicyViolation {
                reason: format!(
                    "session spend {} exceeds soul limit {}",
                    self.economic_belief.session_spend_micro_credits,
                    policy.economic_limits.max_spend_per_session_micro_credits,
                ),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_within_ceiling_succeeds() {
        let mut belief = AgentBelief::default();
        let policy = PolicyManifest::default(); // Ceiling includes "chat:*"

        let grant = GrantedCapability {
            capability: "chat:send".into(),
            granted_by: "server-1".into(),
            granted_at: Utc::now(),
            expires_at: None,
            constraints: vec![],
        };

        assert!(belief.grant_capability(grant, &policy).is_ok());
        assert!(belief.has_capability("chat:send"));
    }

    #[test]
    fn grant_beyond_ceiling_fails() {
        let mut belief = AgentBelief::default();
        let policy = PolicyManifest::default(); // No "admin:*" in ceiling

        let grant = GrantedCapability {
            capability: "admin:delete".into(),
            granted_by: "server-1".into(),
            granted_at: Utc::now(),
            expires_at: None,
            constraints: vec![],
        };

        assert!(belief.grant_capability(grant, &policy).is_err());
    }

    #[test]
    fn trust_increases_on_success() {
        let mut belief = AgentBelief::default();
        belief.update_trust("peer-1", true);
        belief.update_trust("peer-1", true);

        let score = &belief.trust_scores["peer-1"];
        assert!(score.score > 0.5);
        assert_eq!(score.successful_interactions, 2);
    }

    #[test]
    fn trust_decreases_on_failure() {
        let mut belief = AgentBelief::default();
        belief.update_trust("peer-1", true);
        belief.update_trust("peer-1", true);
        let high = belief.trust_scores["peer-1"].score;

        belief.update_trust("peer-1", false);
        let after = belief.trust_scores["peer-1"].score;

        assert!(after < high);
    }

    #[test]
    fn trust_decays_over_time() {
        let mut score = TrustScore {
            score: 0.9,
            successful_interactions: 10,
            failed_interactions: 0,
            last_interaction: Utc::now() - chrono::Duration::days(60),
        };

        score.decay(Utc::now());

        // After 60 days (2 half-lives), score should be closer to 0.5
        assert!(score.score < 0.75);
        assert!(score.score > 0.5);
    }

    #[test]
    fn revoke_removes_capability() {
        let mut belief = AgentBelief::default();
        let policy = PolicyManifest::default();

        let grant = GrantedCapability {
            capability: "chat:send".into(),
            granted_by: "server-1".into(),
            granted_at: Utc::now(),
            expires_at: None,
            constraints: vec![],
        };

        belief.grant_capability(grant, &policy).unwrap();
        assert!(belief.has_capability("chat:send"));

        belief.revoke_capability("chat:send");
        assert!(!belief.has_capability("chat:send"));
    }

    #[test]
    fn expired_capability_not_active() {
        let mut belief = AgentBelief::default();
        let policy = PolicyManifest::default();

        let grant = GrantedCapability {
            capability: "chat:send".into(),
            granted_by: "server-1".into(),
            granted_at: Utc::now(),
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
            constraints: vec![],
        };

        belief.grant_capability(grant, &policy).unwrap();
        assert!(!belief.has_capability("chat:send"));
    }

    // ── Knowledge awareness tests ──────────────────────────────────────

    #[test]
    fn serde_roundtrip_with_knowledge_fields() {
        let mut belief = AgentBelief::default();
        belief.record_knowledge_access("rust-async", 0.7);
        belief.record_knowledge_gap("x402-protocol".into(), 0.9, "task_failure".into());

        let json = serde_json::to_string(&belief).unwrap();
        let deserialized: AgentBelief = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.knowledge_relevance["rust-async"], 0.7);
        assert_eq!(deserialized.knowledge_gaps.len(), 1);
        assert_eq!(deserialized.knowledge_gaps[0].topic, "x402-protocol");
        assert!((deserialized.knowledge_gaps[0].priority - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn backwards_compat_deserialize_without_knowledge_fields() {
        // Simulate old JSON that lacks the new fields entirely.
        let old_json = serde_json::json!({
            "capabilities": [],
            "constraints": [],
            "trust_scores": {},
            "reputation": { "overall": 0.5, "tasks_completed": 0, "tasks_failed": 0, "violations": 0 },
            "economic_belief": {
                "balance_micro_credits": 0,
                "burn_rate_per_hour": 0.0,
                "hours_until_exhaustion": null,
                "economic_mode": "sovereign",
                "session_spend_micro_credits": 0
            },
            "last_updated": "2026-01-01T00:00:00Z",
            "last_event_seq": 42
        });

        let belief: AgentBelief = serde_json::from_value(old_json).unwrap();
        assert!(belief.knowledge_relevance.is_empty());
        assert!(belief.knowledge_gaps.is_empty());
        assert_eq!(belief.last_event_seq, 42);
    }

    #[test]
    fn knowledge_access_boosts_and_clamps() {
        let mut belief = AgentBelief::default();

        belief.record_knowledge_access("topic-a", 0.3);
        assert!((belief.knowledge_relevance["topic-a"] - 0.3).abs() < f64::EPSILON);

        belief.record_knowledge_access("topic-a", 0.5);
        assert!((belief.knowledge_relevance["topic-a"] - 0.8).abs() < f64::EPSILON);

        // Should clamp at 1.0
        belief.record_knowledge_access("topic-a", 0.5);
        assert!((belief.knowledge_relevance["topic-a"] - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn knowledge_gap_no_duplicate() {
        let mut belief = AgentBelief::default();

        belief.record_knowledge_gap("topic-x".into(), 0.8, "source-1".into());
        belief.record_knowledge_gap("topic-x".into(), 0.9, "source-2".into());

        assert_eq!(belief.knowledge_gaps.len(), 1);
        // First insertion wins
        assert_eq!(belief.knowledge_gaps[0].source, "source-1");
    }

    #[test]
    fn resolve_knowledge_gap_removes() {
        let mut belief = AgentBelief::default();

        belief.record_knowledge_gap("gap-a".into(), 0.5, "src".into());
        belief.record_knowledge_gap("gap-b".into(), 0.7, "src".into());
        assert_eq!(belief.knowledge_gaps.len(), 2);

        belief.resolve_knowledge_gap("gap-a");
        assert_eq!(belief.knowledge_gaps.len(), 1);
        assert_eq!(belief.knowledge_gaps[0].topic, "gap-b");

        // Resolving a non-existent gap is a no-op
        belief.resolve_knowledge_gap("gap-nonexistent");
        assert_eq!(belief.knowledge_gaps.len(), 1);
    }

    #[test]
    fn top_knowledge_topics_sorts_correctly() {
        let mut belief = AgentBelief::default();

        belief.record_knowledge_access("low", 0.1);
        belief.record_knowledge_access("high", 0.9);
        belief.record_knowledge_access("mid", 0.5);

        let top2 = belief.top_knowledge_topics(2);
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].0, "high");
        assert!((top2[0].1 - 0.9).abs() < f64::EPSILON);
        assert_eq!(top2[1].0, "mid");
        assert!((top2[1].1 - 0.5).abs() < f64::EPSILON);

        // k larger than entries returns all
        let all = belief.top_knowledge_topics(100);
        assert_eq!(all.len(), 3);
    }
}
