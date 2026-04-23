//! Resource budgets and usage accounting for kernel-tier metering.
//!
//! This module holds the type surface consumed by the (future) `BudgetGatePort`
//! trait (lands in BRO-849) and emitted as payload on `kernel.dispatch.completed`
//! events. Types are additive-only: consumers that do not care about budgets
//! treat every field as optional and ignore any variant they do not recognize.

use serde::{Deserialize, Serialize};

/// Resource limits that can constrain a single dispatch or fork.
///
/// All fields are optional — `None` means no limit for that dimension. The
/// type is used both as a "cost hint" supplied on a `KernelContext` and as
/// the authoritative cap checked by a `BudgetGatePort`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cpu_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mem_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_egress_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_syscalls: Option<u64>,
}

/// Actual resource consumption reported by a backend after a dispatch.
///
/// Field accuracy varies by backend — see [`UsageConfidence`] for the
/// accompanying signal. Consumers should treat fields with
/// `UsageConfidence::Unknown` as missing rather than zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub cpu_ms: u64,
    pub mem_peak_kb: u64,
    pub egress_bytes: u64,
    pub duration_ms: u64,
    pub syscall_count: u64,
    pub confidence: UsageConfidence,
}

/// Per-backend accuracy signal for [`ResourceUsage`] fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UsageConfidence {
    /// Actually measured at the hypervisor/syscall boundary.
    Measured,
    /// Approximated from available proxies (e.g., wall-clock for CPU).
    #[default]
    Estimated,
    /// Backend did not report this field.
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_budget_defaults_to_no_limits() {
        let b = ResourceBudget::default();
        assert!(b.max_cpu_ms.is_none());
        assert!(b.max_mem_kb.is_none());
        assert!(b.max_egress_bytes.is_none());
        assert!(b.max_duration_ms.is_none());
        assert!(b.max_syscalls.is_none());
    }

    #[test]
    fn resource_budget_default_omits_none_fields() {
        // Confirms serde(skip_serializing_if) is wired correctly so a fully
        // unconstrained budget does not pollute the wire format.
        let b = ResourceBudget::default();
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn resource_budget_partial_roundtrip() {
        let b = ResourceBudget {
            max_cpu_ms: Some(1_000),
            max_duration_ms: Some(30_000),
            ..Default::default()
        };
        let json = serde_json::to_string(&b).unwrap();
        let back: ResourceBudget = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn resource_usage_roundtrip() {
        let u = ResourceUsage {
            cpu_ms: 100,
            mem_peak_kb: 2048,
            egress_bytes: 0,
            duration_ms: 120,
            syscall_count: 42,
            confidence: UsageConfidence::Measured,
        };
        let json = serde_json::to_string(&u).unwrap();
        let back: ResourceUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(u, back);
    }

    #[test]
    fn usage_confidence_defaults_to_estimated() {
        assert_eq!(UsageConfidence::default(), UsageConfidence::Estimated);
    }

    #[test]
    fn usage_confidence_serde_snake_case() {
        let json = serde_json::to_string(&UsageConfidence::Measured).unwrap();
        assert_eq!(json, "\"measured\"");
        let back: UsageConfidence = serde_json::from_str("\"unknown\"").unwrap();
        assert_eq!(back, UsageConfidence::Unknown);
    }
}
