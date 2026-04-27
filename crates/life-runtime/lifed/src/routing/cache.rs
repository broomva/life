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
        if let Some((_, entry)) = self.by_sid.remove(&sid.value) {
            let user_id = entry.read().user_id.clone();
            if let Some(mut sids) = self.by_user.get_mut(&user_id) {
                sids.retain(|s| s != &sid.value);
            }
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
            self.evict(&aios_v1::SessionId { value: sid });
        }
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
            self.evict(&aios_v1::SessionId { value: sid });
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
}
