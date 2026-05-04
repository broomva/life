//! Lyapunov function V_k stubs for each level.
//!
//! See spec §3 for the chosen physical proxies:
//!
//! - V_0 — plant: tool-latency residual + drop rate + provider health
//! - V_1 — autonomic: operational residual + context pressure + economic drift
//! - V_2 — meta-control: rule-set diff + shadow-eval veto rate + inheritance lag
//! - V_3 — governance: policy violation count + AGENTS.md drift (v1.0)
//!
//! The `SystemSnapshot` is a deliberately opaque holder for now — v0.1 will
//! replace it with a real read of `HomeostaticState` / arcan-core tick stats
//! / autoany rule deltas. Keeping it abstract here lets the trait surface
//! lock in without coupling to those crates from the scaffold.

use serde::{Deserialize, Serialize};

use crate::perturbation::Level;

/// One observation `(t, V_k(t))` of the level-k Lyapunov function.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LyapunovSample {
    /// Wall-clock time (ms since epoch) the sample was taken.
    pub t_ms: u64,
    /// V_k(t) — non-negative scalar, by Lyapunov-function convention.
    pub v: f64,
}

impl LyapunovSample {
    /// Construct a sample, clamping `v` to be non-negative.
    pub fn new(t_ms: u64, v: f64) -> Self {
        Self {
            t_ms,
            v: v.max(0.0),
        }
    }
}

/// Opaque holder for the cross-cutting state needed to compute V_k.
///
/// v0.0: an empty struct, since none of the V_k implementations are wired.
/// v0.1+: real fields populated from `arcan-core` / `autonomic-core` /
/// `autoany-core` reads.
#[derive(Debug, Clone, Default)]
pub struct SystemSnapshot {
    /// Optional placeholder for the L0 read (tool latencies + drops).
    pub l0: Option<L0Probe>,
    /// Optional placeholder for the L1 read (HomeostaticState).
    pub l1: Option<L1Probe>,
    /// Optional placeholder for the L2 read (rule-set delta + vetoes).
    pub l2: Option<L2Probe>,
    /// Optional placeholder for the L3 read (policy + AGENTS.md hashes).
    pub l3: Option<L3Probe>,
}

/// Placeholder L0 probe; real shape lands with v0.1.
#[derive(Debug, Clone, Default)]
pub struct L0Probe {
    pub tool_latency_residual: f64,
    pub request_drop_rate: f64,
    pub provider_health: f64,
}

/// Placeholder L1 probe; real shape will reuse `HomeostaticState`.
#[derive(Debug, Clone, Default)]
pub struct L1Probe {
    pub operational_residual: f64,
    pub context_pressure: f64,
    pub economic_drift: f64,
}

/// Placeholder L2 probe; real shape lands with v1.0.
#[derive(Debug, Clone, Default)]
pub struct L2Probe {
    pub rule_set_diff: f64,
    pub shadow_eval_veto_rate: f64,
    pub inheritance_lag_ms: f64,
}

/// Placeholder L3 probe; real shape lands with v1.0.
#[derive(Debug, Clone, Default)]
pub struct L3Probe {
    pub policy_violation_count: f64,
    pub agents_md_drift: f64,
}

/// Trait every level's V_k computer implements.
pub trait LyapunovFn: Send + Sync {
    /// The level this V_k is defined for.
    fn level(&self) -> Level;
    /// Compute V_k(t) given the snapshot. Always non-negative.
    fn compute(&self, snapshot: &SystemSnapshot) -> f64;
}

/// V_0 — L0 plant Lyapunov. Stub: returns 0.0 until probes are wired.
#[derive(Debug, Default, Clone)]
pub struct V0Plant {
    /// Weight on tool-latency residual.
    pub w_latency: f64,
    /// Weight on request drop rate.
    pub w_drop: f64,
    /// Weight on (1 − provider_health)².
    pub w_health: f64,
}

impl LyapunovFn for V0Plant {
    fn level(&self) -> Level {
        Level::L0
    }
    fn compute(&self, snapshot: &SystemSnapshot) -> f64 {
        let Some(p) = snapshot.l0.as_ref() else {
            return 0.0;
        };
        let h = (1.0 - p.provider_health).max(0.0);
        self.w_latency * p.tool_latency_residual.powi(2)
            + self.w_drop * p.request_drop_rate.max(0.0)
            + self.w_health * h.powi(2)
    }
}

/// V_1 — L1 autonomic Lyapunov. Stub.
#[derive(Debug, Default, Clone)]
pub struct V1Autonomic {
    pub w_op: f64,
    pub w_cog: f64,
    pub w_econ: f64,
}

impl LyapunovFn for V1Autonomic {
    fn level(&self) -> Level {
        Level::L1
    }
    fn compute(&self, snapshot: &SystemSnapshot) -> f64 {
        let Some(p) = snapshot.l1.as_ref() else {
            return 0.0;
        };
        self.w_op * p.operational_residual.powi(2)
            + self.w_cog * p.context_pressure.max(0.0).powi(2)
            + self.w_econ * p.economic_drift.abs()
    }
}

/// V_2 — L2 meta-control Lyapunov. Stub.
#[derive(Debug, Default, Clone)]
pub struct V2Autoany {
    pub w_rule: f64,
    pub w_veto: f64,
    pub w_lag: f64,
}

impl LyapunovFn for V2Autoany {
    fn level(&self) -> Level {
        Level::L2
    }
    fn compute(&self, snapshot: &SystemSnapshot) -> f64 {
        let Some(p) = snapshot.l2.as_ref() else {
            return 0.0;
        };
        self.w_rule * p.rule_set_diff.max(0.0)
            + self.w_veto * p.shadow_eval_veto_rate.powi(2)
            + self.w_lag * p.inheritance_lag_ms.max(0.0)
    }
}

/// V_3 — L3 governance Lyapunov. Stub. (v1.0 target.)
#[derive(Debug, Default, Clone)]
pub struct V3Governance {
    pub w_violations: f64,
    pub w_drift: f64,
}

impl LyapunovFn for V3Governance {
    fn level(&self) -> Level {
        Level::L3
    }
    fn compute(&self, snapshot: &SystemSnapshot) -> f64 {
        let Some(p) = snapshot.l3.as_ref() else {
            return 0.0;
        };
        self.w_violations * p.policy_violation_count.max(0.0)
            + self.w_drift * p.agents_md_drift.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_yields_zero_v() {
        let snap = SystemSnapshot::default();
        assert_eq!(V0Plant::default().compute(&snap), 0.0);
        assert_eq!(V1Autonomic::default().compute(&snap), 0.0);
        assert_eq!(V2Autoany::default().compute(&snap), 0.0);
        assert_eq!(V3Governance::default().compute(&snap), 0.0);
    }

    #[test]
    fn v0_combines_three_terms() {
        let v0 = V0Plant {
            w_latency: 1.0,
            w_drop: 1.0,
            w_health: 1.0,
        };
        let snap = SystemSnapshot {
            l0: Some(L0Probe {
                tool_latency_residual: 2.0,
                request_drop_rate: 0.5,
                provider_health: 0.0,
            }),
            ..SystemSnapshot::default()
        };
        // 4 + 0.5 + 1 = 5.5
        assert!((v0.compute(&snap) - 5.5).abs() < f64::EPSILON);
    }

    #[test]
    fn lyapunov_sample_clamps_negative() {
        let s = LyapunovSample::new(1, -3.0);
        assert_eq!(s.v, 0.0);
    }
}
