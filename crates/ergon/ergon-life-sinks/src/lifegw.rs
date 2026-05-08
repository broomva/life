//! `LifegwSink` — forwards every [`StreamEvent`] to a bounded
//! `tokio::sync::mpsc::Sender<StreamEvent>`.
//!
//! In production: the host runtime (arcan + lifed adapter, BRO-1001 /
//! BRO-1002) creates an mpsc channel at session start, hands the
//! `Sender` end to ergon (wrapped as a `LifegwSink`) and forwards the
//! `Receiver` end to lifegw which encodes events as SSE / Connect-stream
//! frames for the upstream client.
//!
//! ## Backpressure semantics
//!
//! The mpsc channel is bounded. When the consumer is slow:
//!
//! 1. The channel fills.
//! 2. `send()` awaits.
//! 3. The autonomous loop pauses on its next `sink.emit(...).await`.
//! 4. The provider's streaming call is naturally throttled.
//!
//! When the consumer disconnects (`Receiver` dropped or closed),
//! `send()` returns `Err(SendError)`. `LifegwSink` translates that to
//! [`ErgonError::StreamClosed`], which propagates up the autonomous
//! loop and lets the runtime cancel the upstream provider call.
//!
//! ## Why a separate sink (not just `mpsc::Sender`)
//!
//! `StreamSink` is the trait ergon's loop knows about. Wrapping the
//! mpsc in a sink lets it compose via [`ergon::FanoutSink`] alongside
//! `LagoSink` and `VigilSink`. The wrapper also localizes the
//! error-translation logic (`SendError` → `StreamClosed`).
//!
//! ## Channel capacity recommendation
//!
//! Spec §3.10 recommends capacity 64. That accommodates a typical
//! token-streaming burst without falling behind a streaming HTTP
//! consumer. Tune per deployment via [`LifegwSink::new`] which
//! accepts a pre-built `Sender`.

use async_trait::async_trait;
use ergon::{ErgonError, Result, StreamEvent, StreamSink};
use tokio::sync::mpsc;

/// Default mpsc channel capacity recommended by spec §3.10.
pub const DEFAULT_LIFEGW_CHANNEL_CAPACITY: usize = 64;

/// A [`StreamSink`] that forwards every event to a bounded mpsc.
///
/// Constructed with a pre-built `mpsc::Sender<StreamEvent>` (the runtime
/// owns the channel and decides capacity). For convenience,
/// [`LifegwSink::with_default_capacity`] creates the channel for you and
/// returns both the sink and the receiver.
pub struct LifegwSink {
    tx: mpsc::Sender<StreamEvent>,
}

impl LifegwSink {
    /// Construct from a pre-built mpsc sender. Use this when the runtime
    /// owns the channel (typical production path — lifed creates the
    /// channel, hands the receiver to lifegw, the sender to ergon).
    pub fn new(tx: mpsc::Sender<StreamEvent>) -> Self {
        Self { tx }
    }

    /// Convenience: build a sink + receiver pair with the default
    /// capacity ([`DEFAULT_LIFEGW_CHANNEL_CAPACITY`]).
    pub fn with_default_capacity() -> (Self, mpsc::Receiver<StreamEvent>) {
        Self::with_capacity(DEFAULT_LIFEGW_CHANNEL_CAPACITY)
    }

    /// Convenience: build a sink + receiver pair with the given capacity.
    pub fn with_capacity(capacity: usize) -> (Self, mpsc::Receiver<StreamEvent>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { tx }, rx)
    }

    /// Number of slots currently free in the channel. Useful for
    /// instrumentation. Returns 0 if the channel is closed.
    pub fn capacity(&self) -> usize {
        self.tx.capacity()
    }

    /// True iff the receiver has been dropped — subsequent `emit` calls
    /// will return [`ErgonError::StreamClosed`].
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

impl std::fmt::Debug for LifegwSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifegwSink")
            .field("capacity", &self.tx.capacity())
            .field("closed", &self.tx.is_closed())
            .finish()
    }
}

#[async_trait]
impl StreamSink for LifegwSink {
    async fn emit(&self, event: StreamEvent) -> Result<()> {
        match self.tx.send(event).await {
            Ok(()) => Ok(()),
            Err(_) => Err(ErgonError::StreamClosed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ergon::StopReason;

    fn done_event() -> StreamEvent {
        StreamEvent::Done {
            stop_reason: StopReason::EndTurn,
        }
    }

    #[tokio::test]
    async fn emit_forwards_to_receiver() {
        let (sink, mut rx) = LifegwSink::with_capacity(4);
        sink.emit(done_event()).await.expect("emit ok");
        let received = rx.recv().await.expect("got event");
        match received {
            StreamEvent::Done { stop_reason } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[tokio::test]
    async fn emit_after_receiver_dropped_returns_stream_closed() {
        let (sink, rx) = LifegwSink::with_capacity(4);
        drop(rx);
        let err = sink.emit(done_event()).await.expect_err("should err");
        assert!(matches!(err, ErgonError::StreamClosed));
    }

    #[tokio::test]
    async fn capacity_decreases_with_pending_events() {
        let (sink, _rx) = LifegwSink::with_capacity(4);
        let initial = sink.capacity();
        // initial == 4 (no events queued yet)
        assert_eq!(initial, 4);
        sink.emit(done_event()).await.expect("ok");
        sink.emit(done_event()).await.expect("ok");
        // After 2 emits with no consumer, 2 slots are filled.
        assert_eq!(sink.capacity(), 2);
    }

    #[tokio::test]
    async fn full_channel_blocks_until_consumer_drains() {
        let (sink, mut rx) = LifegwSink::with_capacity(2);
        // Fill the channel to capacity.
        sink.emit(done_event()).await.expect("ok");
        sink.emit(done_event()).await.expect("ok");
        assert_eq!(sink.capacity(), 0);

        // The next emit would block. Race a drain to unblock it within
        // a short window.
        let drainer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let _ = rx.recv().await;
            rx
        });
        // This will only complete once the drainer pops one event.
        sink.emit(done_event()).await.expect("ok after drain");
        let _rx = drainer.await.expect("join");
    }

    #[tokio::test]
    async fn is_closed_reflects_receiver_drop() {
        let (sink, rx) = LifegwSink::with_capacity(4);
        assert!(!sink.is_closed());
        drop(rx);
        assert!(sink.is_closed());
    }

    #[tokio::test]
    async fn with_default_capacity_returns_sink_and_receiver() {
        let (sink, mut rx) = LifegwSink::with_default_capacity();
        assert_eq!(sink.capacity(), DEFAULT_LIFEGW_CHANNEL_CAPACITY);
        sink.emit(done_event()).await.expect("ok");
        let _ = rx.recv().await.expect("got event");
    }

    #[test]
    fn debug_print_shows_capacity_and_closed() {
        let (sink, _rx) = LifegwSink::with_capacity(8);
        let s = format!("{sink:?}");
        assert!(s.contains("capacity"));
        assert!(s.contains("closed"));
    }
}
