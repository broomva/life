//! Perturbation taxonomy — typed enum of every controlled perturbation we
//! plan to support across the four RCS levels.
//!
//! See the design spec at
//! `docs/superpowers/specs/2026-05-04-life-perturb-design.md` §4 for the
//! per-level mechanism table and §5 for the full API rationale.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// One of the four RCS hierarchy levels.
///
/// Matches the canonical levels in
/// `research/rcs/data/parameters.toml` and the paper's Theorem 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Level {
    /// L0 — external plant. Arcan agent loop / provider boundary.
    L0,
    /// L1 — agent internal. Autonomic homeostatic state.
    L1,
    /// L2 — meta-control. EGRI / autoany rule loop.
    L2,
    /// L3 — governance. policy.yaml + AGENTS.md drift.
    L3,
}

impl Level {
    /// String form matching the canonical TOML id (`"L0"`..`"L3"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::L0 => "L0",
            Level::L1 => "L1",
            Level::L2 => "L2",
            Level::L3 => "L3",
        }
    }
}

/// Severity hint — a coarse grade for perturbations whose amplitude is
/// not naturally numeric (e.g. `BadRuleInjection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Mild,
    Moderate,
    Severe,
}

/// Stable identifier for a single perturbation instance, used to thread
/// telemetry across inject → record → fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PerturbationId(pub Ulid);

impl PerturbationId {
    /// Generate a fresh ULID-based id.
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for PerturbationId {
    fn default() -> Self {
        Self::new()
    }
}

/// All perturbation kinds we plan to support, partitioned by target level
/// in the variant ordering. v0.0 scaffold — most variants are unimplemented
/// in the injectors.
///
/// See spec §4 for the per-level injection contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PerturbationKind {
    // ─── L0 — external plant ──────────────────────────────────────────
    /// Force the provider boundary into a 429-storm at the given rate
    /// for the given duration.
    RateLimitStorm { rps: f64, duration: Duration },
    /// Add latency jitter to every tool call.
    ToolLatencyJitter {
        mean_ms: u32,
        jitter_ms: u32,
        duration: Duration,
    },
    /// Drop a fraction of requests outright.
    RequestDrop { drop_prob: f64, duration: Duration },
    /// Simulate memory corruption at the L0 boundary.
    MemoryCorrupt { bytes_flipped: u32 },

    // ─── L1 — autonomic ──────────────────────────────────────────────
    /// Skew the token budget projection to drive a mode change.
    BudgetSkew { token_delta_pct: f64 },
    /// Force rapid economic-mode flapping past the hysteresis gate.
    ModeFlapInduction {
        transitions_per_sec: f64,
        duration: Duration,
    },
    /// Override the hysteresis gate's `min_hold_ms`.
    HysteresisOverride { new_min_hold_ms: u64 },

    // ─── L2 — meta-control ───────────────────────────────────────────
    /// Inject a controlled-bad rule into the L2 rule set, bypassing
    /// shadow eval. Used to test recovery via veto.
    BadRuleInjection {
        rule_text: String,
        severity: Severity,
    },
    /// Disable shadow eval for the given window.
    ShadowEvalDisable { duration: Duration },
    /// Thrash a list of policy keys at the given churn rate.
    PolicyThrash { keys: Vec<String>, churn_hz: f64 },

    // ─── L3 — governance (deferred to v1.0) ──────────────────────────
    /// Flip a single key in a sandbox copy of `.control/policy.yaml`.
    GovernanceFlip {
        policy_path: String,
        key: String,
        new_value: serde_json::Value,
    },
}

impl PerturbationKind {
    /// The level this kind targets.
    pub fn target_level(&self) -> Level {
        match self {
            PerturbationKind::RateLimitStorm { .. }
            | PerturbationKind::ToolLatencyJitter { .. }
            | PerturbationKind::RequestDrop { .. }
            | PerturbationKind::MemoryCorrupt { .. } => Level::L0,
            PerturbationKind::BudgetSkew { .. }
            | PerturbationKind::ModeFlapInduction { .. }
            | PerturbationKind::HysteresisOverride { .. } => Level::L1,
            PerturbationKind::BadRuleInjection { .. }
            | PerturbationKind::ShadowEvalDisable { .. }
            | PerturbationKind::PolicyThrash { .. } => Level::L2,
            PerturbationKind::GovernanceFlip { .. } => Level::L3,
        }
    }

    /// Stable short name suitable for use as a span attribute or metric
    /// label.
    pub fn name(&self) -> &'static str {
        match self {
            PerturbationKind::RateLimitStorm { .. } => "RateLimitStorm",
            PerturbationKind::ToolLatencyJitter { .. } => "ToolLatencyJitter",
            PerturbationKind::RequestDrop { .. } => "RequestDrop",
            PerturbationKind::MemoryCorrupt { .. } => "MemoryCorrupt",
            PerturbationKind::BudgetSkew { .. } => "BudgetSkew",
            PerturbationKind::ModeFlapInduction { .. } => "ModeFlapInduction",
            PerturbationKind::HysteresisOverride { .. } => "HysteresisOverride",
            PerturbationKind::BadRuleInjection { .. } => "BadRuleInjection",
            PerturbationKind::ShadowEvalDisable { .. } => "ShadowEvalDisable",
            PerturbationKind::PolicyThrash { .. } => "PolicyThrash",
            PerturbationKind::GovernanceFlip { .. } => "GovernanceFlip",
        }
    }
}

/// One concrete perturbation instance: id + level + kind + scheduled
/// integration window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Perturbation {
    pub id: PerturbationId,
    pub level: Level,
    pub kind: PerturbationKind,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Total integration window. Some kinds carry their own duration; this
    /// field is the *scheduled* window enforced by the campaign runner.
    pub duration: Duration,
}

impl Perturbation {
    /// Construct a perturbation whose level matches the kind's
    /// [`PerturbationKind::target_level`].
    pub fn new(kind: PerturbationKind, duration: Duration) -> Self {
        Self {
            id: PerturbationId::new(),
            level: kind.target_level(),
            kind,
            started_at: chrono::Utc::now(),
            duration,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_levels_match_taxonomy() {
        assert_eq!(
            PerturbationKind::RateLimitStorm {
                rps: 100.0,
                duration: Duration::from_secs(1),
            }
            .target_level(),
            Level::L0
        );
        assert_eq!(
            PerturbationKind::BudgetSkew {
                token_delta_pct: 0.5
            }
            .target_level(),
            Level::L1
        );
        assert_eq!(
            PerturbationKind::ShadowEvalDisable {
                duration: Duration::from_secs(1),
            }
            .target_level(),
            Level::L2
        );
        assert_eq!(
            PerturbationKind::GovernanceFlip {
                policy_path: "/tmp/policy.yaml".into(),
                key: "max_tokens".into(),
                new_value: serde_json::Value::Null,
            }
            .target_level(),
            Level::L3
        );
    }

    #[test]
    fn perturbation_new_pairs_level_and_kind() {
        let p = Perturbation::new(
            PerturbationKind::RateLimitStorm {
                rps: 50.0,
                duration: Duration::from_secs(10),
            },
            Duration::from_secs(60),
        );
        assert_eq!(p.level, Level::L0);
        assert_eq!(p.kind.name(), "RateLimitStorm");
    }

    #[test]
    fn level_as_str_matches_canonical() {
        assert_eq!(Level::L0.as_str(), "L0");
        assert_eq!(Level::L1.as_str(), "L1");
        assert_eq!(Level::L2.as_str(), "L2");
        assert_eq!(Level::L3.as_str(), "L3");
    }
}
