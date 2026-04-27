//! Active-session blocklist + snapshot publisher.
//!
//! Spec C₂ §5.4: lifed maintains an in-memory blocklist of revoked
//! session ids. Sub-phase B publishes the set as a JSON file at
//! `cfg.auth.revoked_sids_path`; substrates poll the file every 30 s
//! (revocation gap bounded by Tier-3 expiry of 30 s). Spec C₆ replaces
//! the snapshot with a server-streamed `RevokedSessionStream`.

use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::RwLock;

use aios_proto::aios::v1 as aios_v1;

#[derive(Default)]
pub struct RevokedSidSet {
    inner: Arc<RwLock<HashSet<String>>>,
}

impl RevokedSidSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, sid: &aios_v1::SessionId) {
        self.inner.write().insert(sid.value.clone());
    }

    pub fn contains(&self, sid: &aios_v1::SessionId) -> bool {
        self.inner.read().contains(&sid.value)
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.inner.read().iter().cloned().collect()
    }

    /// Sub-phase B: write the snapshot to a file substrates can poll.
    /// Spec C₆ replaces with a server-streamed `RevokedSessionStream`.
    pub fn write_snapshot_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        let snap = self.snapshot();
        let json = serde_json::to_string(&snap).unwrap_or_else(|_| "[]".to_string());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, json)
    }
}
