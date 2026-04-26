//! Multi-tab fan-out registry. Sub-phase A scaffolds the registry shape;
//! B12 wires the real fanout pump.

use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::mpsc;

use life_runtime_proto::life::v1 as pb;

#[derive(Default)]
pub struct FanoutRegistry {
    senders: Arc<RwLock<Vec<mpsc::Sender<Result<pb::AgentEvent, tonic::Status>>>>>,
}

impl FanoutRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    /// Register a downstream sender. Returns the registry index.
    pub fn register(&self, tx: mpsc::Sender<Result<pb::AgentEvent, tonic::Status>>) -> usize {
        let mut guard = self.senders.write();
        let i = guard.len();
        guard.push(tx);
        i
    }
}
