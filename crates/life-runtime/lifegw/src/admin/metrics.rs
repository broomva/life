//! Admin-plane operational counters. Sub-phase E sweep (item #13).
//!
//! Per Spec C₃ §13 (admin metrics):
//! - `gateway.admin.connection_total` — every accepted UDS connection.
//! - `gateway.admin.rejected_total{reason}` — denied requests by
//!   reason. Reasons:
//!     - `peercred` — SO_PEERCRED returned an error (kernel refused).
//!     - `group` — peer's primary GID does not match `admin_gid` AND
//!       supplementary lookup did not place them in `admin_gid`.
//!     - `protocol` — request shape was invalid (missing extension,
//!       malformed body).
//!     - `group_lookup` — Sub-phase E sweep (item #13): the
//!       supplementary-group lookup itself failed (uid not in
//!       `/etc/passwd`, getgrouplist syscall errored). Fail-CLOSED
//!       semantics — the request is denied and the operator gets a
//!       counter advance so dashboards can alert on misconfigurations.
//! - `gateway.blocklist.size` — current entry count.
//! - `gateway.blocklist.match_total` — public-plane matches.
//!
//! The implementation uses `AtomicU64` counters for now. A future
//! Sub-phase E continuation will wire OTLP via `life_vigil` once the
//! gateway-side metrics-exporter glue is in place; the counter shape
//! here is forward-compatible — switching to OTLP `Counter`s requires
//! changing the recorder, not the call sites.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Spec C₃ §13 admin-plane metric counters.
///
/// Cheap to clone (Arc-wrapped); pass by value to handlers that need
/// to record events.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct AdminMetrics {
    inner: Arc<AdminMetricsInner>,
}

#[derive(Debug, Default)]
struct AdminMetricsInner {
    connection_total: AtomicU64,
    rejected_peercred: AtomicU64,
    rejected_group: AtomicU64,
    rejected_protocol: AtomicU64,
    /// Sub-phase E sweep (item #13): group-lookup failure counter.
    /// Bumps when the supplementary-group lookup syscall errors AND
    /// the request is fail-closed denied.
    rejected_group_lookup: AtomicU64,
    blocklist_size: AtomicU64,
    blocklist_match_total: AtomicU64,
}

impl AdminMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_connection(&self) {
        self.inner.connection_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_total(&self) -> u64 {
        self.inner.connection_total.load(Ordering::Relaxed)
    }

    pub fn record_rejection(&self, reason: RejectReason) {
        let counter = match reason {
            RejectReason::Peercred => &self.inner.rejected_peercred,
            RejectReason::Group => &self.inner.rejected_group,
            RejectReason::Protocol => &self.inner.rejected_protocol,
            RejectReason::GroupLookup => &self.inner.rejected_group_lookup,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn rejected_total(&self, reason: RejectReason) -> u64 {
        let counter = match reason {
            RejectReason::Peercred => &self.inner.rejected_peercred,
            RejectReason::Group => &self.inner.rejected_group,
            RejectReason::Protocol => &self.inner.rejected_protocol,
            RejectReason::GroupLookup => &self.inner.rejected_group_lookup,
        };
        counter.load(Ordering::Relaxed)
    }

    pub fn set_blocklist_size(&self, n: u64) {
        self.inner.blocklist_size.store(n, Ordering::Relaxed);
    }

    pub fn blocklist_size(&self) -> u64 {
        self.inner.blocklist_size.load(Ordering::Relaxed)
    }

    pub fn record_blocklist_match(&self) {
        self.inner
            .blocklist_match_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn blocklist_match_total(&self) -> u64 {
        self.inner.blocklist_match_total.load(Ordering::Relaxed)
    }
}

/// Spec C₃ §13 admin reject reasons. The `{reason}` label values on
/// `gateway.admin.rejected_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RejectReason {
    Peercred,
    Group,
    Protocol,
    /// Sub-phase E sweep (item #13).
    GroupLookup,
}

impl RejectReason {
    pub fn as_label(self) -> &'static str {
        match self {
            RejectReason::Peercred => "peercred",
            RejectReason::Group => "group",
            RejectReason::Protocol => "protocol",
            RejectReason::GroupLookup => "group_lookup",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_metrics_counter_increments() {
        let m = AdminMetrics::new();
        m.record_connection();
        m.record_connection();
        assert_eq!(m.connection_total(), 2);
    }

    #[test]
    fn admin_metrics_per_reason_isolated() {
        let m = AdminMetrics::new();
        m.record_rejection(RejectReason::Peercred);
        m.record_rejection(RejectReason::GroupLookup);
        m.record_rejection(RejectReason::GroupLookup);
        assert_eq!(m.rejected_total(RejectReason::Peercred), 1);
        assert_eq!(m.rejected_total(RejectReason::GroupLookup), 2);
        assert_eq!(m.rejected_total(RejectReason::Group), 0);
        assert_eq!(m.rejected_total(RejectReason::Protocol), 0);
    }

    #[test]
    fn admin_metrics_clone_shares_state() {
        let m = AdminMetrics::new();
        let m2 = m.clone();
        m.record_connection();
        m2.record_connection();
        assert_eq!(m.connection_total(), 2);
        assert_eq!(m2.connection_total(), 2);
    }

    #[test]
    fn reject_reason_labels_match_spec() {
        assert_eq!(RejectReason::Peercred.as_label(), "peercred");
        assert_eq!(RejectReason::Group.as_label(), "group");
        assert_eq!(RejectReason::Protocol.as_label(), "protocol");
        assert_eq!(RejectReason::GroupLookup.as_label(), "group_lookup");
    }
}
