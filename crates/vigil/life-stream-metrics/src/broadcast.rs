//! [`MeasuredSender`] / [`MeasuredReceiver`] — a transparent instrumentation
//! wrapper around `tokio::sync::broadcast`.
//!
//! The public API mirrors the subset of `tokio::sync::broadcast` that the
//! Life substrate uses (`send` / `subscribe` / `recv` / `try_recv` /
//! `resubscribe` / `len` / `receiver_count`), so migrating a channel is a
//! type swap, not a rewrite. The error types (`RecvError`, `TryRecvError`)
//! are re-exported verbatim, so existing `match` arms keep compiling.
//!
//! Internally the wrapper carries an [`Envelope`] with a publish `Instant`
//! so it can emit `stream.broadcast.delta_latency_seconds` on every drain —
//! the one metric a call-site instrumentation can't produce without threading
//! a timestamp through the payload. The envelope is invisible to callers:
//! `send` takes a `T` and `recv` yields a `T`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::broadcast;

use crate::metrics::StreamMetrics;

pub use tokio::sync::broadcast::error::{RecvError, SendError, TryRecvError};

/// Internal transport wrapper carrying the publish timestamp alongside the
/// payload so the receiver can compute publish→consume latency.
#[derive(Clone)]
struct Envelope<T> {
    value: T,
    published_at: Instant,
}

/// The sending half of a measured broadcast channel.
///
/// Clone is cheap (shares the inner `broadcast::Sender`, the metric bundle,
/// and the consumer-id counter).
#[derive(Clone)]
pub struct MeasuredSender<T> {
    inner: broadcast::Sender<Envelope<T>>,
    metrics: Arc<StreamMetrics>,
    capacity: usize,
    next_consumer_id: Arc<AtomicU64>,
}

/// The receiving half of a measured broadcast channel.
pub struct MeasuredReceiver<T> {
    inner: broadcast::Receiver<Envelope<T>>,
    metrics: Arc<StreamMetrics>,
    consumer_id: String,
    channel: String,
    next_consumer_id: Arc<AtomicU64>,
}

/// Create a measured broadcast channel with `capacity`, labelled `channel`,
/// using the global meter provider.
pub fn measured_channel<T: Clone>(
    capacity: usize,
    channel: impl Into<String>,
) -> (MeasuredSender<T>, MeasuredReceiver<T>) {
    let metrics = Arc::new(StreamMetrics::new(channel));
    measured_channel_with(capacity, metrics)
}

/// Create a measured broadcast channel with `capacity`, reusing an existing
/// [`StreamMetrics`] bundle (used by tests to bind to an in-memory reader).
pub fn measured_channel_with<T: Clone>(
    capacity: usize,
    metrics: Arc<StreamMetrics>,
) -> (MeasuredSender<T>, MeasuredReceiver<T>) {
    let (inner_tx, inner_rx) = broadcast::channel::<Envelope<T>>(capacity);
    let next_consumer_id = Arc::new(AtomicU64::new(0));
    let channel = metrics.channel().to_string();
    let consumer_id = consumer_id(&channel, &next_consumer_id);
    let tx = MeasuredSender {
        inner: inner_tx,
        metrics: Arc::clone(&metrics),
        capacity,
        next_consumer_id: Arc::clone(&next_consumer_id),
    };
    let rx = MeasuredReceiver {
        inner: inner_rx,
        metrics,
        consumer_id,
        channel,
        next_consumer_id,
    };
    (tx, rx)
}

fn consumer_id(channel: &str, counter: &AtomicU64) -> String {
    let ordinal = counter.fetch_add(1, Ordering::Relaxed);
    format!("{channel}#{ordinal}")
}

impl<T: Clone> MeasuredSender<T> {
    /// Broadcast `value`. On success, records the publish, the current
    /// saturation (`len / capacity`), and the live consumer count. Mirrors
    /// `broadcast::Sender::send`, returning the number of receivers.
    ///
    /// `event_type` is an optional low-cardinality tag for the published
    /// metric; pass `None` when the payload has no meaningful type.
    pub fn send_typed(&self, value: T, event_type: Option<&str>) -> Result<usize, SendError<T>> {
        let envelope = Envelope {
            value,
            published_at: Instant::now(),
        };
        match self.inner.send(envelope) {
            Ok(n) => {
                self.metrics.on_published(event_type);
                self.metrics.set_saturation(self.inner.len(), self.capacity);
                self.metrics.set_consumer_count(self.inner.receiver_count());
                Ok(n)
            }
            Err(SendError(env)) => Err(SendError(env.value)),
        }
    }

    /// Broadcast `value` with no event-type label. See [`Self::send_typed`].
    pub fn send(&self, value: T) -> Result<usize, SendError<T>> {
        self.send_typed(value, None)
    }

    /// Subscribe a new receiver, assigning it a fresh per-channel id.
    pub fn subscribe(&self) -> MeasuredReceiver<T> {
        let channel = self.metrics.channel().to_string();
        let consumer_id = consumer_id(&channel, &self.next_consumer_id);
        MeasuredReceiver {
            inner: self.inner.subscribe(),
            metrics: Arc::clone(&self.metrics),
            consumer_id,
            channel,
            next_consumer_id: Arc::clone(&self.next_consumer_id),
        }
    }

    /// Number of queued messages the slowest receiver has not yet consumed.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the channel currently holds no un-consumed messages.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Number of live receivers.
    pub fn receiver_count(&self) -> usize {
        self.inner.receiver_count()
    }

    /// The metric bundle backing this channel (shared with all receivers).
    pub fn metrics(&self) -> &Arc<StreamMetrics> {
        &self.metrics
    }
}

impl<T: Clone> MeasuredReceiver<T> {
    /// The id assigned to this receiver (`<channel>#<ordinal>`).
    pub fn consumer_id(&self) -> &str {
        &self.consumer_id
    }

    fn on_ok(&self, published_at: Instant, event_type: Option<&str>) {
        self.metrics.on_consumed(&self.consumer_id, event_type);
        self.metrics
            .record_delta_latency(event_type, published_at.elapsed().as_secs_f64());
    }

    /// Await the next value. Records a drain + latency on success and a lag on
    /// `RecvError::Lagged`. The error is returned unchanged so callers keep
    /// their existing match arms.
    pub async fn recv(&mut self) -> Result<T, RecvError> {
        match self.inner.recv().await {
            Ok(env) => {
                self.on_ok(env.published_at, None);
                Ok(env.value)
            }
            Err(RecvError::Lagged(skipped)) => {
                self.metrics.on_lagged(&self.consumer_id, skipped);
                Err(RecvError::Lagged(skipped))
            }
            Err(other) => Err(other),
        }
    }

    /// Non-blocking receive. Same instrumentation as [`Self::recv`].
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        match self.inner.try_recv() {
            Ok(env) => {
                self.on_ok(env.published_at, None);
                Ok(env.value)
            }
            Err(TryRecvError::Lagged(skipped)) => {
                self.metrics.on_lagged(&self.consumer_id, skipped);
                Err(TryRecvError::Lagged(skipped))
            }
            Err(other) => Err(other),
        }
    }

    /// Re-subscribe from the current tail, assigning a fresh consumer id.
    pub fn resubscribe(&self) -> MeasuredReceiver<T> {
        let consumer_id = consumer_id(&self.channel, &self.next_consumer_id);
        MeasuredReceiver {
            inner: self.inner.resubscribe(),
            metrics: Arc::clone(&self.metrics),
            consumer_id,
            channel: self.channel.clone(),
            next_consumer_id: Arc::clone(&self.next_consumer_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_payload_transparently() {
        let (tx, mut rx) = measured_channel::<u32>(8, "test.roundtrip");
        tx.send(7).expect("send");
        assert_eq!(rx.recv().await.expect("recv"), 7);
    }

    #[tokio::test]
    async fn subscribe_and_resubscribe_assign_distinct_ids() {
        let (tx, rx) = measured_channel::<u32>(8, "test.ids");
        let r2 = tx.subscribe();
        let r3 = rx.resubscribe();
        assert_ne!(rx.consumer_id(), r2.consumer_id());
        assert_ne!(rx.consumer_id(), r3.consumer_id());
        assert_ne!(r2.consumer_id(), r3.consumer_id());
    }

    #[tokio::test]
    async fn lagged_surfaces_unchanged_to_the_caller() {
        // Capacity 2, produce 4 with no drain → first recv sees Lagged(2).
        let (tx, mut rx) = measured_channel::<u32>(2, "test.lag");
        for i in 0..4 {
            tx.send(i).expect("send");
        }
        match rx.recv().await {
            Err(RecvError::Lagged(skipped)) => assert_eq!(skipped, 2),
            other => panic!("expected Lagged(2), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_reports_receiver_count() {
        let (tx, _rx) = measured_channel::<u32>(4, "test.count");
        // one receiver from channel creation
        assert_eq!(tx.send(1).expect("send"), 1);
        let _r2 = tx.subscribe();
        assert_eq!(tx.send(2).expect("send"), 2);
    }
}
