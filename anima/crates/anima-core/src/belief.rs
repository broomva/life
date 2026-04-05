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
}
