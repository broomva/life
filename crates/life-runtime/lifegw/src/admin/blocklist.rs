//! In-memory IP / user blocklist. Sub-phase D (D2).
//!
//! Operators can deny outright (separate from the rate limiter, which
//! throttles). Entries are in-process; restart resets to empty. Per
//! Spec C₃ §3.6 the blocklist is the lifegw admin plane's "Blocklist"
//! RPC family — both `ip:<addr>` and `user:<user_id>` subjects are
//! supported under a single subject string.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;

#[derive(Debug, Clone)]
pub struct BlocklistEntry {
    pub subject: String,
    pub reason: String,
    pub added_at: SystemTime,
}

/// Concurrent-safe blocklist registry.
#[derive(Default)]
pub struct Blocklist {
    inner: Arc<RwLock<HashMap<String, BlocklistEntry>>>,
}

impl Blocklist {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cheap clone — shares the inner registry.
    pub fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn add(&self, subject: impl Into<String>, reason: impl Into<String>) {
        let entry = BlocklistEntry {
            subject: subject.into(),
            reason: reason.into(),
            added_at: SystemTime::now(),
        };
        self.inner.write().insert(entry.subject.clone(), entry);
    }

    pub fn remove(&self, subject: &str) -> bool {
        self.inner.write().remove(subject).is_some()
    }

    pub fn contains(&self, subject: &str) -> bool {
        self.inner.read().contains_key(subject)
    }

    pub fn list(&self) -> Vec<BlocklistEntry> {
        self.inner.read().values().cloned().collect()
    }

    /// Convenience: check whether a `user:<id>` subject is blocked.
    pub fn user_blocked(&self, user_id: &str) -> bool {
        self.contains(&format!("user:{user_id}"))
    }

    /// Convenience: check whether an `ip:<addr>` subject is blocked.
    pub fn ip_blocked(&self, ip: std::net::IpAddr) -> bool {
        self.contains(&format!("ip:{ip}"))
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }
}

impl Clone for Blocklist {
    fn clone(&self) -> Self {
        self.handle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn add_remove_round_trip() {
        let b = Blocklist::new();
        assert!(b.is_empty());
        b.add("user:alice", "abusive ratio");
        assert!(b.contains("user:alice"));
        assert!(b.user_blocked("alice"));
        assert_eq!(b.len(), 1);
        assert!(b.remove("user:alice"));
        assert!(!b.contains("user:alice"));
    }

    #[test]
    fn ip_blocked_helper() {
        let b = Blocklist::new();
        let ip = std::net::IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        b.add(format!("ip:{ip}"), "scraper");
        assert!(b.ip_blocked(ip));
        assert!(!b.ip_blocked(std::net::IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8))));
    }

    #[test]
    fn list_returns_all_entries() {
        let b = Blocklist::new();
        b.add("user:a", "r1");
        b.add("user:b", "r2");
        let entries = b.list();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn handle_shares_registry() {
        let b1 = Blocklist::new();
        let b2 = b1.handle();
        b1.add("user:shared", "r");
        assert!(b2.contains("user:shared"));
    }

    #[test]
    fn remove_missing_returns_false() {
        let b = Blocklist::new();
        assert!(!b.remove("user:never-added"));
    }
}
