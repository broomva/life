//! Lago-backed idempotency-key dedup backend.
//!
//! Spec C₂ §3.6: dedup tuple `(user, project, key, method)` with 24 h TTL.
//! Production path. The lago substrate handles TTL itself, so `sweep` is
//! a no-op for this backend.

use std::sync::Arc;

use async_trait::async_trait;
use tonic::Status;

use lago_proxy::LagoCall;

use super::{IdemKey, IdempotencyStore};

pub struct LagoBackedStore {
    pub lago: Arc<dyn LagoCall>,
}

impl LagoBackedStore {
    pub fn new(lago: Arc<dyn LagoCall>) -> Self {
        Self { lago }
    }
}

#[async_trait]
impl IdempotencyStore for LagoBackedStore {
    async fn lookup(&self, key: &IdemKey) -> Result<Option<Vec<u8>>, Status> {
        self.lago
            .idem_lookup(&key.as_bytes())
            .await
            .map_err(|e| e.into())
    }

    async fn persist(&self, key: IdemKey, response: Vec<u8>) -> Result<(), Status> {
        self.lago
            .idem_persist(&key.as_bytes(), response)
            .await
            .map_err(|e| e.into())
    }

    fn sweep(&self) {
        // Lago handles TTL itself; nothing to do here.
    }
}
