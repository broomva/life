//! In-memory idempotency-key dedup backend.
//!
//! Spec C₂ §3.6: dedup tuple `(user, project, key, method)` with 24 h TTL.
//! Sub-phase B keeps this backend for tests + the dev daemon path; the
//! lago-backed backend (sibling module) is the production path.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use tonic::Status;

use super::{IdemKey, IdempotencyStore};

pub struct InMemoryStore {
    map: DashMap<IdemKey, (Instant, Vec<u8>)>,
    ttl: Duration,
}

impl InMemoryStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            map: DashMap::new(),
            ttl,
        }
    }
}

#[async_trait]
impl IdempotencyStore for InMemoryStore {
    async fn lookup(&self, key: &IdemKey) -> Result<Option<Vec<u8>>, Status> {
        Ok(self.map.get(key).and_then(|e| {
            let (at, bytes) = e.value();
            if at.elapsed() > self.ttl {
                None
            } else {
                Some(bytes.clone())
            }
        }))
    }

    async fn persist(&self, key: IdemKey, response: Vec<u8>) -> Result<(), Status> {
        self.map.insert(key, (Instant::now(), response));
        Ok(())
    }

    fn sweep(&self) {
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

    #[tokio::test]
    async fn lookup_returns_persisted_value_within_ttl() {
        let store = InMemoryStore::new(Duration::from_secs(60));
        store.persist(k("k1"), b"hello".to_vec()).await.unwrap();
        assert_eq!(
            store.lookup(&k("k1")).await.unwrap().as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(store.lookup(&k("missing")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn sweep_drops_zero_ttl_entries() {
        let store = InMemoryStore::new(Duration::from_nanos(1));
        store.persist(k("k1"), b"x".to_vec()).await.unwrap();
        std::thread::sleep(Duration::from_millis(5));
        store.sweep();
        assert!(store.lookup(&k("k1")).await.unwrap().is_none());
    }
}
