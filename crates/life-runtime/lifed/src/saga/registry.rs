//! In-memory registry of inflight + recently-completed sagas.
//!
//! Spec C₂ §4.1: `lifed` keeps a tabular record of every saga as it
//! progresses, so the admin-plane `Saga.ListInflight` / `Saga.Show` RPCs
//! have something to read. Sub-phase C ships an in-memory record only;
//! the lago `system/lifed/saga/<saga_id>` namespace persists the same
//! event stream so historical sagas can be reconstructed.
//!
//! Design notes:
//! - `DashMap<saga_id, SagaRecord>` for sharded concurrent access.
//! - Records stay in-memory after completion until evicted by the
//!   30-minute TTL sweeper (C₆ adds the sweeper; for sub-phase C
//!   completed sagas accumulate slowly enough that a fixed bound + LRU
//!   eviction hasn't been needed yet).

use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;

use aios_proto::aios::v1 as aios_v1;

#[derive(Clone, Debug)]
pub struct SagaRecord {
    pub saga_id: String,
    pub saga_kind: String,
    pub sid: aios_v1::SessionId,
    pub started_at: Instant,
    pub current_step: String,
    pub completed_steps: Vec<String>,
    pub compensations_applied: Vec<String>,
    pub status: SagaStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SagaStatus {
    Inflight,
    Succeeded,
    Compensated,
    Failed,
}

impl SagaStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SagaStatus::Inflight => "inflight",
            SagaStatus::Succeeded => "succeeded",
            SagaStatus::Compensated => "compensated",
            SagaStatus::Failed => "failed",
        }
    }
}

#[derive(Default, Clone)]
pub struct SagaRegistry {
    map: Arc<DashMap<String, SagaRecord>>,
}

impl SagaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a new record for an inflight saga.
    pub fn open(&self, saga_id: &str, kind: &str, sid: &aios_v1::SessionId) {
        self.map.insert(
            saga_id.to_string(),
            SagaRecord {
                saga_id: saga_id.to_string(),
                saga_kind: kind.to_string(),
                sid: sid.clone(),
                started_at: Instant::now(),
                current_step: String::new(),
                completed_steps: Vec::new(),
                compensations_applied: Vec::new(),
                status: SagaStatus::Inflight,
            },
        );
    }

    pub fn step_started(&self, saga_id: &str, step: &str) {
        if let Some(mut r) = self.map.get_mut(saga_id) {
            r.current_step = step.to_string();
        }
    }

    pub fn step_completed(&self, saga_id: &str, step: &str) {
        if let Some(mut r) = self.map.get_mut(saga_id) {
            r.completed_steps.push(step.to_string());
        }
    }

    pub fn compensation_applied(&self, saga_id: &str, step: &str) {
        if let Some(mut r) = self.map.get_mut(saga_id) {
            r.compensations_applied.push(step.to_string());
        }
    }

    pub fn close(&self, saga_id: &str, status: SagaStatus) {
        if let Some(mut r) = self.map.get_mut(saga_id) {
            r.status = status;
        }
    }

    pub fn get(&self, saga_id: &str) -> Option<SagaRecord> {
        self.map.get(saga_id).map(|e| e.value().clone())
    }

    pub fn snapshot_inflight(&self, limit: usize) -> Vec<SagaRecord> {
        self.map
            .iter()
            .filter(|e| e.value().status == SagaStatus::Inflight)
            .take(limit)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Sub-phase E: count of in-flight (not-yet-closed) sagas. Used by
    /// the `life.saga.inflight{kind}` gauge per Spec C₂ §9.3.
    pub fn inflight_count(&self) -> usize {
        self.map
            .iter()
            .filter(|e| e.value().status == SagaStatus::Inflight)
            .count()
    }

    /// Total record count (inflight + completed). Used by tests.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(s: &str) -> aios_v1::SessionId {
        aios_v1::SessionId {
            value: s.to_string(),
        }
    }

    #[test]
    fn open_records_an_inflight_saga() {
        let r = SagaRegistry::new();
        r.open("s1", "create-session", &sid("session-1"));
        let rec = r.get("s1").expect("present");
        assert_eq!(rec.saga_id, "s1");
        assert_eq!(rec.saga_kind, "create-session");
        assert_eq!(rec.status, SagaStatus::Inflight);
        assert_eq!(r.snapshot_inflight(10).len(), 1);
    }

    #[test]
    fn step_lifecycle_is_tracked() {
        let r = SagaRegistry::new();
        r.open("s1", "x", &sid("a"));
        r.step_started("s1", "step-a");
        r.step_completed("s1", "step-a");
        r.step_started("s1", "step-b");
        let rec = r.get("s1").expect("present");
        assert_eq!(rec.current_step, "step-b");
        assert_eq!(rec.completed_steps, vec!["step-a".to_string()]);
    }

    #[test]
    fn close_drops_from_inflight_snapshot() {
        let r = SagaRegistry::new();
        r.open("s1", "x", &sid("a"));
        r.close("s1", SagaStatus::Succeeded);
        assert_eq!(r.snapshot_inflight(10).len(), 0);
        assert_eq!(r.get("s1").unwrap().status, SagaStatus::Succeeded);
    }

    #[test]
    fn snapshot_limit_caps_results() {
        let r = SagaRegistry::new();
        for i in 0..5 {
            r.open(&format!("s{i}"), "x", &sid("a"));
        }
        assert_eq!(r.snapshot_inflight(3).len(), 3);
    }
}
