//! PolicyManifest — the immutable value system of an agent's soul.
//!
//! A PolicyManifest defines what an agent will *never* violate,
//! regardless of what capabilities it is granted or what beliefs it forms.
//! It is the hard boundary that even Autonomic cannot override.
//!
//! Think of it as constitutional law — capabilities are statutes that can
//! be granted and revoked, but the constitution is ratified at birth.

use serde::{Deserialize, Serialize};

/// The immutable value system embedded in an agent's soul.
///
/// Once created, a PolicyManifest cannot be modified. It constrains all
/// downstream behavior: beliefs cannot violate it, capabilities cannot
/// exceed it, and Autonomic treats it as a hard safety shield.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyManifest {
    /// Actions the agent must never perform, regardless of instructions.
    ///
    /// These are absolute prohibitions — no capability grant, economic
    /// incentive, or external pressure can override them.
    ///
    /// Examples: "delete production data", "impersonate a human",
    /// "bypass safety checks", "send funds without approval"
    pub safety_constraints: Vec<SafetyConstraint>,

    /// The maximum set of capabilities this agent can ever hold.
    ///
    /// Even if a server grants a broader capability, the agent must
    /// intersect it with this ceiling. This prevents privilege escalation
    /// beyond the soul's original intent.
    pub capability_ceiling: Vec<String>,

    /// Hard economic limits that Haima must enforce.
    pub economic_limits: EconomicLimits,

    /// Communication policies governing who and what.
    pub communication_policy: CommunicationPolicy,
}

/// A single safety constraint — something the agent must never do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SafetyConstraint {
    /// Machine-readable identifier for this constraint.
    /// Example: "no-impersonation", "no-unaudited-transfers"
    pub id: String,

    /// Human-readable description of the constraint.
    pub description: String,

    /// Severity if violated. Determines response:
    /// - Critical: halt immediately, revoke agent
    /// - High: halt current action, alert operator
    /// - Medium: log and warn, continue with caution
    pub severity: ConstraintSeverity,
}

/// How severe a constraint violation is.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ConstraintSeverity {
    Medium,
    High,
    Critical,
}

/// Hard economic limits embedded in the soul.
///
/// These are lifetime limits, not per-session — they define the
/// economic envelope the agent can ever operate within.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EconomicLimits {
    /// Maximum micro-credits the agent can spend in a single transaction.
    /// 1 USDC = 1,000,000 micro-credits.
    pub max_spend_per_tx_micro_credits: i64,

    /// Maximum micro-credits the agent can spend per session.
    pub max_spend_per_session_micro_credits: i64,

    /// Maximum lifetime spend. None means unlimited (dangerous).
    pub max_lifetime_spend_micro_credits: Option<i64>,

    /// Maximum risk tolerance for economic decisions.
    /// Maps to Autonomic's economic modes.
    pub max_risk_tolerance: RiskTolerance,
}

/// How much economic risk the agent is allowed to take.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum RiskTolerance {
    /// Only spend when certain of value.
    Conservative,
    /// Normal operation — spend within policy auto-approve caps.
    Moderate,
    /// Allowed to take calculated risks for higher returns.
    Aggressive,
}

/// Rules governing who the agent can communicate with and what it can disclose.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommunicationPolicy {
    /// Agent IDs this agent is allowed to communicate with.
    /// Empty means unrestricted.
    pub allowed_peers: Vec<String>,

    /// Topics or data categories that must not be disclosed.
    pub disclosure_restrictions: Vec<String>,

    /// Whether the agent may initiate contact with unknown agents.
    pub allow_unsolicited_contact: bool,
}

impl Default for PolicyManifest {
    fn default() -> Self {
        Self {
            safety_constraints: vec![
                SafetyConstraint {
                    id: "no-impersonation".into(),
                    description: "Must not impersonate a human or another agent".into(),
                    severity: ConstraintSeverity::Critical,
                },
                SafetyConstraint {
                    id: "no-unaudited-transfers".into(),
                    description: "Must not transfer funds without audit trail".into(),
                    severity: ConstraintSeverity::Critical,
                },
            ],
            capability_ceiling: vec![
                "chat:*".into(),
                "knowledge:read".into(),
                "tool:execute".into(),
            ],
            economic_limits: EconomicLimits {
                max_spend_per_tx_micro_credits: 1_000_000,       // $1.00
                max_spend_per_session_micro_credits: 10_000_000, // $10.00
                max_lifetime_spend_micro_credits: None,
                max_risk_tolerance: RiskTolerance::Moderate,
            },
            communication_policy: CommunicationPolicy {
                allowed_peers: vec![],
                disclosure_restrictions: vec![],
                allow_unsolicited_contact: true,
            },
        }
    }
}

impl PolicyManifest {
    /// Check whether a capability is within the soul's ceiling.
    ///
    /// Uses glob-style matching: "chat:*" matches "chat:send", "chat:stream", etc.
    pub fn allows_capability(&self, capability: &str) -> bool {
        if self.capability_ceiling.is_empty() {
            return true; // No ceiling means unrestricted
        }

        self.capability_ceiling.iter().any(|pattern| {
            if pattern.ends_with(":*") {
                let prefix = &pattern[..pattern.len() - 1];
                capability.starts_with(prefix)
            } else {
                pattern == capability
            }
        })
    }

    /// Check whether a spend amount is within economic limits for a single transaction.
    pub fn allows_spend(&self, micro_credits: i64) -> bool {
        micro_credits <= self.economic_limits.max_spend_per_tx_micro_credits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_allows_chat() {
        let policy = PolicyManifest::default();
        assert!(policy.allows_capability("chat:send"));
        assert!(policy.allows_capability("chat:stream"));
        assert!(policy.allows_capability("knowledge:read"));
    }

    #[test]
    fn default_policy_denies_unknown() {
        let policy = PolicyManifest::default();
        assert!(!policy.allows_capability("admin:delete"));
        assert!(!policy.allows_capability("payments:initiate"));
    }

    #[test]
    fn spend_within_limits() {
        let policy = PolicyManifest::default();
        assert!(policy.allows_spend(500_000)); // $0.50
        assert!(policy.allows_spend(1_000_000)); // $1.00 (exactly at limit)
        assert!(!policy.allows_spend(1_000_001)); // $1.000001 (over)
    }

    #[test]
    fn empty_ceiling_means_unrestricted() {
        let policy = PolicyManifest {
            capability_ceiling: vec![],
            ..Default::default()
        };
        assert!(policy.allows_capability("anything:at:all"));
    }
}
