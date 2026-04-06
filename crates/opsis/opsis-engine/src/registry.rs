//! Client registry — tracks connected clients and their subscriptions.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use opsis_core::subscription::{ClientId, Subscription};

/// Thread-safe registry of connected clients.
#[derive(Debug, Clone)]
pub struct ClientRegistry {
    inner: Arc<RwLock<HashMap<ClientId, Subscription>>>,
}

impl ClientRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a client with a subscription filter.
    pub async fn register(&self, id: ClientId, sub: Subscription) {
        self.inner.write().await.insert(id, sub);
    }

    /// Remove a client.
    pub async fn unregister(&self, id: &ClientId) {
        self.inner.write().await.remove(id);
    }

    /// Number of currently connected clients.
    pub async fn client_count(&self) -> usize {
        self.inner.read().await.len()
    }
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}
