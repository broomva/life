//! Multi-tab fan-out registry. Sub-phase A scaffolds the registry shape;
//! B12 wires the real fanout pump.

use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::mpsc;

use life_runtime_proto::life::v1 as pb;

/// Type alias for the per-tab AgentEvent sender. Sub-phase A keeps the
/// queue bounded by the receiver-side mpsc capacity (no slow-consumer
/// policy yet — that lands in D4).
pub type AgentEventSender = mpsc::Sender<Result<pb::AgentEvent, tonic::Status>>;

#[derive(Default)]
pub struct FanoutRegistry {
    senders: Arc<RwLock<Vec<AgentEventSender>>>,
}

impl FanoutRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    /// Register a downstream sender. Returns the registry index.
    pub fn register(&self, tx: AgentEventSender) -> usize {
        let mut guard = self.senders.write();
        let i = guard.len();
        guard.push(tx);
        i
    }
}
