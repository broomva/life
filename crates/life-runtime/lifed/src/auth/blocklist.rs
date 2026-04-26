//! Active-session blocklist. Sub-phase A in-memory only; B14 wires
//! `revoked_sids.json` snapshot + reload.

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
}
