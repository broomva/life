//! Idempotency-key dedup store.
//!
//! Spec C₂ §3.6: handlers that mutate must check the
//! `(user, project, idempotency-key, method)` tuple and return the cached
//! response on replay. Sub-phase B ships two backends:
//!
//! - `in_memory::InMemoryStore` — DashMap with TTL sweeper. Tests + dev.
//! - `lago_store::LagoBackedStore` — durable dedup via lago. Production.

pub mod in_memory;
pub mod lago_store;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct IdemKey {
    pub user_id: String,
    pub project_id: String,
    pub key: String,
    pub method: String,
}

impl IdemKey {
    /// Wire-encoding used by lago-backed storage. Pipe-delimited so the
    /// substrate sees a deterministic byte string.
    pub fn as_bytes(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}",
            self.user_id, self.project_id, self.method, self.key
        )
        .into_bytes()
    }
}

#[async_trait]
pub trait IdempotencyStore: Send + Sync {
    async fn lookup(&self, key: &IdemKey) -> Result<Option<Vec<u8>>, tonic::Status>;
    async fn persist(&self, key: IdemKey, response: Vec<u8>) -> Result<(), tonic::Status>;
    fn sweep(&self);
}

/// Convenience constructor — returns an `Arc<dyn IdempotencyStore>` backed by
/// the in-memory implementation. Used by tests and the dev daemon path.
pub fn boxed_in_memory(ttl: Duration) -> Arc<dyn IdempotencyStore> {
    Arc::new(in_memory::InMemoryStore::new(ttl))
}
