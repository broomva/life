//! Idempotency-key dedup store. Sub-phase A in-memory; B7 swaps to lago.
//!
//! Spec C₂ §3.6: dedup tuple `(user, project, key, method)` with 24 h TTL.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct IdemKey {
    pub user_id: String,
    pub project_id: String,
    pub key: String,
    pub method: String,
}

pub struct IdempotencyStore {
    map: DashMap<IdemKey, (Instant, Vec<u8>)>,
    ttl: Duration,
}

impl IdempotencyStore {
    pub fn new(ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            map: DashMap::new(),
            ttl,
        })
    }

    pub fn lookup(&self, key: &IdemKey) -> Option<Vec<u8>> {
        self.map.get(key).and_then(|e| {
            let (at, bytes) = e.value();
            if at.elapsed() > self.ttl {
                None
            } else {
                Some(bytes.clone())
            }
        })
    }

    pub fn persist(&self, key: IdemKey, response_bytes: Vec<u8>) {
        self.map.insert(key, (Instant::now(), response_bytes));
    }

    /// Sweep expired entries.
    pub fn sweep(&self) {
        let now = Instant::now();
        let ttl = self.ttl;
        self.map.retain(|_, (at, _)| now.duration_since(*at) <= ttl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> IdemKey {
        IdemKey {
            user_id: "alice".into(),
            project_id: "p1".into(),
            key: s.into(),
            method: "Wallet.Debit".into(),
        }
    }

    #[test]
    fn lookup_returns_persisted_value_within_ttl() {
        let store = IdempotencyStore::new(Duration::from_secs(60));
        store.persist(k("k1"), b"hello".to_vec());
        assert_eq!(store.lookup(&k("k1")).as_deref(), Some(&b"hello"[..]));
        assert_eq!(store.lookup(&k("missing")), None);
    }

    #[test]
    fn sweep_drops_zero_ttl_entries() {
        let store = IdempotencyStore::new(Duration::from_nanos(1));
        store.persist(k("k1"), b"x".to_vec());
        std::thread::sleep(Duration::from_millis(5));
        store.sweep();
        assert!(store.lookup(&k("k1")).is_none());
    }
}
