//! In-memory routing cache. Sub-phase A ships the basic shape; eviction lands
//! in B8; cold-start replay from lago lands in D2.
//!
//! Per Spec C₂ §6.1, the cache maps `SessionId → RouteEntry`. Multi-tab
//! fanout senders are stored per entry. Locking uses DashMap on the outer
//! map and parking_lot::RwLock on entries (ergonomic; tokio::sync::RwLock
//! would be required only when await is held across the lock).

use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use parking_lot::RwLock;

use aios_proto::aios::v1 as aios_v1;
use lago_proxy::LagoCall;

use super::fanout::FanoutRegistry;

/// Routing cache — DashMap-backed sharded outer map; parking_lot::RwLock per entry.
pub struct RoutingCache {
    by_sid: DashMap<String, Arc<RwLock<RouteEntry>>>,
    by_user: DashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub sid: aios_v1::SessionId,
    pub user_id: String,
    pub project_id: String,
    pub agent_id: String,
    pub lago_namespace: String,
    pub haima_wallet: String,
    pub anima_account: String,
    pub last_touched: Instant,
    pub status: SessionStatus,
    /// Per-session multi-tab fan-out registry. Spec C₂ §6.4.
    pub fanout: Arc<FanoutRegistry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Detached,
    Hibernated,
}

/// Flat snapshot of one routing-cache entry. Used by admin-plane
/// `Runtime.SessionsListAll` + `RoutingCache.Dump`. Includes the
/// downstream addresses (lago_namespace, haima_wallet, anima_account) so a
/// single snapshot satisfies both RPCs without re-locking the entry.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub sid: aios_v1::SessionId,
    pub user_id: String,
    pub project_id: String,
    pub agent_id: String,
    pub lago_namespace: String,
    pub haima_wallet: String,
    pub anima_account: String,
    pub last_touched: Instant,
    pub status: SessionStatus,
    pub attached_streams: u32,
}

impl RoutingCache {
    pub fn new() -> Self {
        Self {
            by_sid: DashMap::new(),
            by_user: DashMap::new(),
        }
    }

    /// Insert a minimal entry. Each entry owns a fresh `FanoutRegistry`
    /// so SendMessage and StreamSession can broadcast to all attached
    /// tabs.
    pub fn insert_minimal(&self, sid: &aios_v1::SessionId, user_id: &str, project_id: &str) {
        let entry = RouteEntry {
            sid: sid.clone(),
            user_id: user_id.to_string(),
            project_id: project_id.to_string(),
            agent_id: format!("agent-{}", sid.value),
            lago_namespace: format!("session/{}", sid.value),
            haima_wallet: format!("wallet-{}", sid.value),
            anima_account: format!("account-{user_id}"),
            last_touched: Instant::now(),
            status: SessionStatus::Active,
            fanout: Arc::new(FanoutRegistry::new()),
        };
        self.by_sid
            .insert(sid.value.clone(), Arc::new(RwLock::new(entry)));
        self.by_user
            .entry(user_id.to_string())
            .or_default()
            .push(sid.value.clone());
        // Sub-phase E: metric series wiring per Spec C₂ §9.3.
        crate::observability::metrics::record_session_created("Tier-2");
        crate::observability::metrics::set_cache_size(self.by_sid.len() as i64);
        crate::observability::metrics::set_session_active(self.by_sid.len() as i64, "Tier-2");
    }

    /// Return the per-session fan-out registry. Sub-phase B's
    /// `SendMessage` / `StreamSession` handlers attach to this so a
    /// single substrate dispatch reaches every connected tab.
    pub fn lookup_fanout(&self, sid: &aios_v1::SessionId) -> Option<Arc<FanoutRegistry>> {
        self.by_sid
            .get(&sid.value)
            .map(|e| Arc::clone(&e.read().fanout))
    }

    pub fn lookup(&self, sid: &aios_v1::SessionId) -> Option<RouteEntry> {
        self.by_sid.get(&sid.value).map(|e| e.read().clone())
    }

    pub fn evict(&self, sid: &aios_v1::SessionId) {
        self.evict_with_reason(sid, "explicit");
    }

    /// Sub-phase E: internal eviction with metric label. The
    /// `cache.evictions_total{reason}` series distinguishes
    /// `explicit` / `idle` / `lru` / `revoked` per Spec C₂ §9.3.
    fn evict_with_reason(&self, sid: &aios_v1::SessionId, reason: &str) {
        if let Some((_, entry)) = self.by_sid.remove(&sid.value) {
            let user_id = entry.read().user_id.clone();
            if let Some(mut sids) = self.by_user.get_mut(&user_id) {
                sids.retain(|s| s != &sid.value);
            }
            crate::observability::metrics::record_session_destroyed("Tier-2");
            crate::observability::metrics::record_cache_eviction(reason);
            crate::observability::metrics::set_cache_size(self.by_sid.len() as i64);
            crate::observability::metrics::set_session_active(self.by_sid.len() as i64, "Tier-2");
        }
    }

    pub fn list_for_user(&self, user_id: &str) -> Vec<aios_v1::SessionId> {
        self.by_user
            .get(user_id)
            .map(|v| {
                v.iter()
                    .map(|s| aios_v1::SessionId { value: s.clone() })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn size(&self) -> usize {
        self.by_sid.len()
    }

    /// Mark a session's status — used by the eviction sweeper to pick
    /// detached sessions for idle-eviction.
    pub fn mark_status(&self, sid: &aios_v1::SessionId, status: SessionStatus) {
        if let Some(entry) = self.by_sid.get(&sid.value) {
            entry.write().status = status;
        }
    }

    /// Snapshot the cache as `SessionSummary` records for admin-plane
    /// `Runtime.SessionsListAll` + `RoutingCache.Dump`. `attached_streams`
    /// is read from each entry's fan-out registry.
    pub fn snapshot_summaries(&self, limit: usize) -> Vec<SessionSummary> {
        self.by_sid
            .iter()
            .take(limit)
            .map(|e| {
                let g = e.value().read();
                SessionSummary {
                    sid: g.sid.clone(),
                    user_id: g.user_id.clone(),
                    project_id: g.project_id.clone(),
                    agent_id: g.agent_id.clone(),
                    lago_namespace: g.lago_namespace.clone(),
                    haima_wallet: g.haima_wallet.clone(),
                    anima_account: g.anima_account.clone(),
                    last_touched: g.last_touched,
                    status: g.status,
                    attached_streams: g.fanout.len() as u32,
                }
            })
            .collect()
    }

    /// Test helper: backdate `last_touched` so eviction logic can be
    /// exercised deterministically. Hidden from rustdoc and not part of
    /// the public sub-phase API.
    #[doc(hidden)]
    pub fn touch_back(&self, sid: &aios_v1::SessionId, when: std::time::Instant) {
        if let Some(entry) = self.by_sid.get(&sid.value) {
            entry.write().last_touched = when;
        }
    }

    /// Evict entries that have been Detached and idle for more than
    /// `threshold`. Spec C₂ §6.3 — operators tune via
    /// `cfg.routing.idle_threshold_secs`.
    pub fn evict_idle(&self, threshold: std::time::Duration) {
        let now = std::time::Instant::now();
        let to_evict: Vec<String> = self
            .by_sid
            .iter()
            .filter_map(|e| {
                let entry = e.value().read();
                if entry.status == SessionStatus::Detached
                    && now.duration_since(entry.last_touched) > threshold
                {
                    Some(entry.sid.value.clone())
                } else {
                    None
                }
            })
            .collect();
        for sid in to_evict {
            self.evict_with_reason(&aios_v1::SessionId { value: sid }, "idle");
        }
    }

    /// Sub-phase D2: warm the cache from lago by enumerating the
    /// `session/*` namespace prefix. Each namespace `session/<sid>`
    /// becomes a routing-cache entry minted with placeholder
    /// `(user_id, project_id)` pairs derived from the namespace
    /// — the live entry is fully populated on the next user-facing
    /// `Agent.CreateSession` or `Agent.DescribeSession` call (which
    /// re-runs the `OpenLagoNamespace` saga step idempotently).
    ///
    /// Returns the number of sessions warmed. When `lago.ListNamespaces`
    /// is unavailable (the lago daemon predates Spec C₂ §4.1's typed
    /// RPC), the call returns 0 and the cache populates lazily on
    /// first traffic per session — which is the documented Spec C₂
    /// §6.3 fallback path.
    pub async fn cold_start(&self, lago: Arc<dyn LagoCall>) -> Result<u32, tonic::Status> {
        let started = std::time::Instant::now();
        let prefix = "session/";
        let namespaces = lago
            .list_namespaces(prefix)
            .await
            .map_err(tonic::Status::from)?;
        let mut warmed = 0u32;
        for ns in namespaces {
            // namespace = "session/<sid>"
            let Some(sid_value) = ns.strip_prefix(prefix) else {
                continue;
            };
            if sid_value.is_empty() {
                continue;
            }
            let sid = aios_v1::SessionId {
                value: sid_value.to_string(),
            };
            // Placeholder fill — the live values land when the next
            // CreateSession/Describe touches this entry.
            self.insert_minimal(&sid, "<cold-start>", "<cold-start>");
            self.mark_status(&sid, SessionStatus::Detached);
            warmed = warmed.saturating_add(1);
        }
        // Sub-phase E: cold-start replay duration metric per Spec C₂ §9.3.
        crate::observability::metrics::record_replay_seconds(started.elapsed().as_secs_f64());
        Ok(warmed)
    }

    /// LRU-evict until under `hard_cap`. Spec C₂ §6.3 — `hard_cap` is
    /// `cfg.routing.hard_cap`. Sorts by `last_touched`, evicts the oldest
    /// excess entries.
    pub fn evict_to_cap(&self, hard_cap: usize) {
        if self.by_sid.len() <= hard_cap {
            return;
        }
        let mut entries: Vec<(String, std::time::Instant)> = self
            .by_sid
            .iter()
            .map(|e| {
                let r = e.value().read();
                (r.sid.value.clone(), r.last_touched)
            })
            .collect();
        entries.sort_by_key(|(_, t)| *t);
        let to_evict = self.by_sid.len() - hard_cap;
        for (sid, _) in entries.into_iter().take(to_evict) {
            self.evict_with_reason(&aios_v1::SessionId { value: sid }, "lru");
        }
    }
}

impl Default for RoutingCache {
    fn default() -> Self {
        Self::new()
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
    fn insert_lookup_evict_round_trip() {
        let cache = RoutingCache::new();
        cache.insert_minimal(&sid("abc"), "alice", "p1");
        let entry = cache.lookup(&sid("abc")).expect("present");
        assert_eq!(entry.user_id, "alice");
        assert_eq!(entry.project_id, "p1");
        assert_eq!(cache.size(), 1);

        let mine = cache.list_for_user("alice");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].value, "abc");

        cache.evict(&sid("abc"));
        assert!(cache.lookup(&sid("abc")).is_none());
        assert_eq!(cache.size(), 0);
        assert!(cache.list_for_user("alice").is_empty());
    }

    #[test]
    fn multiple_sessions_for_one_user() {
        let cache = RoutingCache::new();
        cache.insert_minimal(&sid("a"), "alice", "p1");
        cache.insert_minimal(&sid("b"), "alice", "p2");
        assert_eq!(cache.list_for_user("alice").len(), 2);
    }

    #[test]
    fn eviction_drops_idle_detached_entries() {
        let cache = RoutingCache::new();
        cache.insert_minimal(&sid("idle"), "alice", "p");
        cache.mark_status(&sid("idle"), SessionStatus::Detached);
        cache.touch_back(
            &sid("idle"),
            std::time::Instant::now() - std::time::Duration::from_secs(7200),
        );

        cache.insert_minimal(&sid("active"), "alice", "p");

        cache.evict_idle(std::time::Duration::from_secs(3600));

        assert!(cache.lookup(&sid("idle")).is_none(), "idle evicted");
        assert!(cache.lookup(&sid("active")).is_some(), "active retained");
    }

    #[test]
    fn hard_cap_evicts_lru() {
        let cache = RoutingCache::new();
        for i in 0..5 {
            let s = sid(&format!("s{i}"));
            cache.insert_minimal(&s, "alice", "p");
            cache.mark_status(&s, SessionStatus::Detached);
            cache.touch_back(
                &s,
                std::time::Instant::now() - std::time::Duration::from_secs(60 - 10 * i as u64),
            );
        }
        cache.evict_to_cap(3);
        assert_eq!(cache.size(), 3, "cache size at cap");
        // Oldest two (`s0`, `s1`) should be evicted.
        assert!(cache.lookup(&sid("s0")).is_none());
        assert!(cache.lookup(&sid("s1")).is_none());
        assert!(cache.lookup(&sid("s4")).is_some());
    }

    #[test]
    fn snapshot_summaries_returns_active_entries() {
        let cache = RoutingCache::new();
        cache.insert_minimal(&sid("a"), "alice", "p");
        cache.insert_minimal(&sid("b"), "bob", "q");
        let summaries = cache.snapshot_summaries(100);
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().any(|s| s.user_id == "alice"));
        assert!(summaries.iter().any(|s| s.user_id == "bob"));
        for s in &summaries {
            assert_eq!(s.attached_streams, 0, "no streams attached yet");
        }
    }

    #[test]
    fn snapshot_summaries_respects_limit() {
        let cache = RoutingCache::new();
        for i in 0..5 {
            cache.insert_minimal(&sid(&format!("s{i}")), "alice", "p");
        }
        let summaries = cache.snapshot_summaries(3);
        assert_eq!(summaries.len(), 3);
    }

    #[tokio::test]
    async fn cold_start_warms_cache_from_seeded_lago() {
        let cache = RoutingCache::new();
        let mock_lago = crate::dev_mocks::MockLago::new();
        mock_lago.seed_namespaces(vec![
            "session/abc".to_string(),
            "session/def".to_string(),
            "system/lifed/saga/foo".to_string(), // filtered by prefix
        ]);
        let warmed = cache
            .cold_start(Arc::new(mock_lago))
            .await
            .expect("cold start");
        assert_eq!(warmed, 2, "two session/* namespaces");
        assert_eq!(cache.size(), 2);
        assert!(cache.lookup(&sid("abc")).is_some());
        assert!(cache.lookup(&sid("def")).is_some());
        // Detached so eviction sweeper can claim them once real traffic
        // arrives (or `RebuildFromLago` is rerun).
        assert_eq!(
            cache.lookup(&sid("abc")).unwrap().status,
            SessionStatus::Detached
        );
    }

    #[tokio::test]
    async fn cold_start_returns_zero_when_lago_returns_empty() {
        let cache = RoutingCache::new();
        let mock_lago = crate::dev_mocks::MockLago::new();
        // No seed → empty list → 0 warmed.
        let warmed = cache
            .cold_start(Arc::new(mock_lago))
            .await
            .expect("cold start");
        assert_eq!(warmed, 0);
        assert_eq!(cache.size(), 0);
    }

    #[tokio::test]
    async fn cold_start_propagates_lago_failure() {
        let cache = RoutingCache::new();
        let mock_lago = crate::dev_mocks::MockLago::new();
        mock_lago.set_force_fail(true);
        let err = cache
            .cold_start(Arc::new(mock_lago))
            .await
            .expect_err("must fail");
        assert_eq!(err.code(), tonic::Code::Unavailable);
    }
}
