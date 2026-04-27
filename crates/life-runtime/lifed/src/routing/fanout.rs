//! Multi-tab fan-out registry per Spec C₂ §6.4.
//!
//! Each session in the routing cache owns one `FanoutRegistry`. Each
//! attached tab calls `attach(capacity)` to get a bounded receiver
//! stream; `broadcast` clones the event to every attached sender,
//! garbage-collecting any sender whose receiver has been dropped.

use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use life_runtime_proto::life::v1 as pb;

#[derive(Default, Clone)]
pub struct FanoutRegistry {
    senders: Arc<RwLock<Vec<mpsc::Sender<Result<pb::AgentEvent, tonic::Status>>>>>,
}

impl std::fmt::Debug for FanoutRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FanoutRegistry")
            .field("senders", &self.senders.read().len())
            .finish()
    }
}

impl FanoutRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a new downstream. Returns the receiver the handler hands
    /// back to tonic.
    pub fn attach(&self, capacity: usize) -> ReceiverStream<Result<pb::AgentEvent, tonic::Status>> {
        let (tx, rx) = mpsc::channel(capacity);
        self.senders.write().push(tx);
        ReceiverStream::new(rx)
    }

    /// Broadcast one event to every attached sender. Drops senders whose
    /// receivers have been closed (slow-consumer GC).
    pub fn broadcast(&self, event: pb::AgentEvent) {
        let mut guard = self.senders.write();
        guard.retain(|tx| match tx.try_send(Ok(event.clone())) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => true, // keep — D4 will mark Stalled
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// Number of attached senders. Used by tests / admin introspection.
    pub fn len(&self) -> usize {
        self.senders.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.senders.read().is_empty()
    }
}
