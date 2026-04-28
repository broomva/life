//! Multi-tab fan-out registry per Spec C₂ §6.4 + §8.2.
//!
//! Each session in the routing cache owns one `FanoutRegistry`. Each
//! attached tab calls `attach(capacity)` to get a bounded receiver
//! stream; `broadcast` clones the event to every attached sender.
//!
//! Sub-phase D6 wires the slow-consumer policy: each entry tracks a
//! `full_count`. A sender that returns `Full` more than
//! [`STALLED_THRESHOLD`] times consecutively is GC'd (the slow consumer
//! is dropped, not buffered indefinitely). On every successful send the
//! count resets. The metric `life.daemon.slow_stream_total` increments
//! on each `Full` return so operators can spot saturated tabs.
//!
//! Sub-phase D6 also adds the "one upstream pump per session" guard
//! (`pump_active`): the first SendMessage that arrives spawns a pump;
//! subsequent SendMessage / StreamSession calls reuse the active pump
//! by attaching their downstream sender. This eliminates the two-tab
//! dispatch duplication (sub-phase B review follow-up #5).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use life_runtime_proto::life::v1 as pb;

type AgentEventSender = mpsc::Sender<Result<pb::AgentEvent, tonic::Status>>;

/// Number of consecutive `Full` returns before a sender is GC'd.
/// Spec C₂ §8.2: slow consumers are dropped, never indefinitely buffered.
pub const STALLED_THRESHOLD: u32 = 5;

/// Per-attachment state. Wraps the sender with a full-count atomic.
#[derive(Debug)]
pub struct FanoutEntry {
    pub tx: AgentEventSender,
    pub full_count: AtomicU32,
    pub last_full_at_unix_nanos: AtomicI64,
}

#[derive(Default)]
pub struct FanoutRegistry {
    senders: Arc<RwLock<Vec<Arc<FanoutEntry>>>>,
    /// Sub-phase D6: cumulative `slow_stream_total` so the metric series
    /// can read it without locking.
    slow_stream_total: Arc<AtomicU64>,
    /// Sub-phase D6: set to true when an upstream pump is in flight. The
    /// first SendMessage CAS's false→true; subsequent send_message calls
    /// see true and skip spawning a duplicate pump.
    pump_active: Arc<std::sync::atomic::AtomicBool>,
}

impl Clone for FanoutRegistry {
    fn clone(&self) -> Self {
        Self {
            senders: Arc::clone(&self.senders),
            slow_stream_total: Arc::clone(&self.slow_stream_total),
            pump_active: Arc::clone(&self.pump_active),
        }
    }
}

impl std::fmt::Debug for FanoutRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FanoutRegistry")
            .field("senders", &self.senders.read().len())
            .field(
                "slow_stream_total",
                &self.slow_stream_total.load(Ordering::SeqCst),
            )
            .field("pump_active", &self.pump_active.load(Ordering::SeqCst))
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
        let entry = Arc::new(FanoutEntry {
            tx,
            full_count: AtomicU32::new(0),
            last_full_at_unix_nanos: AtomicI64::new(0),
        });
        self.senders.write().push(entry);
        ReceiverStream::new(rx)
    }

    /// Broadcast one event to every attached sender per Spec C₂ §8.2.
    ///
    /// Behaviour per attachment:
    /// - `Ok(())`: reset full_count, keep the entry.
    /// - `Full`: increment full_count + slow_stream_total. If full_count
    ///   exceeds [`STALLED_THRESHOLD`], drop the entry (slow consumer GC).
    /// - `Closed`: drop the entry (the receiver has gone away).
    pub fn broadcast(&self, event: pb::AgentEvent) {
        let mut to_drop = Vec::new();
        // Take a read lock first; entries are Arc so we can flip
        // counters without needing write access.
        {
            let guard = self.senders.read();
            for (i, entry) in guard.iter().enumerate() {
                match entry.tx.try_send(Ok(event.clone())) {
                    Ok(()) => {
                        entry.full_count.store(0, Ordering::SeqCst);
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        let n = entry.full_count.fetch_add(1, Ordering::SeqCst) + 1;
                        self.slow_stream_total.fetch_add(1, Ordering::SeqCst);
                        entry
                            .last_full_at_unix_nanos
                            .store(unix_nanos_now(), Ordering::SeqCst);
                        if n >= STALLED_THRESHOLD {
                            to_drop.push(i);
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => to_drop.push(i),
                }
            }
        }
        if !to_drop.is_empty() {
            // Drop in reverse so indices stay valid.
            let mut w = self.senders.write();
            for i in to_drop.into_iter().rev() {
                if i < w.len() {
                    w.remove(i);
                }
            }
        }
    }

    /// Number of attached senders. Used by tests / admin introspection.
    pub fn len(&self) -> usize {
        self.senders.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.senders.read().is_empty()
    }

    /// Cumulative slow-stream count, exposed for the metric series.
    pub fn slow_stream_total(&self) -> u64 {
        self.slow_stream_total.load(Ordering::SeqCst)
    }

    /// Sub-phase D6: try to claim the upstream-pump slot. Returns true
    /// if this caller acquired the slot (it owns the pump for this
    /// session); false if a pump was already active.
    pub fn try_claim_pump(&self) -> bool {
        self.pump_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Sub-phase D6: release the pump slot when the upstream pump
    /// finishes. Pair this with [`Self::try_claim_pump`].
    pub fn release_pump(&self) {
        self.pump_active.store(false, Ordering::SeqCst);
    }

    pub fn is_pump_active(&self) -> bool {
        self.pump_active.load(Ordering::SeqCst)
    }
}

fn unix_nanos_now() -> i64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slow_consumer_dropped_after_threshold() {
        let registry = FanoutRegistry::new();
        // Tiny capacity so try_send fills immediately.
        let _stream = registry.attach(1);
        // First broadcast lands in the buffer; subsequent broadcasts
        // hit Full because the receiver never reads.
        for _ in 0..(STALLED_THRESHOLD + 1) {
            registry.broadcast(pb::AgentEvent {
                record: None,
                kind: pb::AgentEventKind::Token as i32,
            });
        }
        // After STALLED_THRESHOLD Full returns the entry is GC'd.
        assert_eq!(registry.len(), 0, "stalled attachment GC'd");
        assert!(registry.slow_stream_total() >= STALLED_THRESHOLD as u64);
    }

    #[tokio::test]
    async fn pump_slot_is_exclusive() {
        let registry = FanoutRegistry::new();
        assert!(registry.try_claim_pump(), "first claim wins");
        assert!(!registry.try_claim_pump(), "second claim loses");
        registry.release_pump();
        assert!(registry.try_claim_pump(), "after release, free again");
    }

    #[tokio::test]
    async fn successful_broadcast_resets_full_count() {
        let registry = FanoutRegistry::new();
        let mut stream = registry.attach(2);
        // Fill the buffer.
        registry.broadcast(pb::AgentEvent {
            record: None,
            kind: pb::AgentEventKind::Token as i32,
        });
        registry.broadcast(pb::AgentEvent {
            record: None,
            kind: pb::AgentEventKind::Token as i32,
        });
        // Hit Full twice.
        registry.broadcast(pb::AgentEvent {
            record: None,
            kind: pb::AgentEventKind::Token as i32,
        });
        registry.broadcast(pb::AgentEvent {
            record: None,
            kind: pb::AgentEventKind::Token as i32,
        });
        // Drain once.
        use futures::StreamExt;
        let _ = stream.next().await;
        // Next broadcast succeeds and resets full_count — entry stays.
        registry.broadcast(pb::AgentEvent {
            record: None,
            kind: pb::AgentEventKind::Token as i32,
        });
        assert_eq!(registry.len(), 1, "entry retained after recovery");
    }
}
