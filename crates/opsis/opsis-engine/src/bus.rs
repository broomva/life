//! Event bus — broadcast channels for distributing world state events.
//!
//! Three channels with different capacities serve different consumers:
//! - **event_tx** (16 384) — all normalised state events
//! - **delta_tx** (256) — per-tick world deltas (SSE consumers)
//! - **fast_path_tx** (1 024) — high-severity events (severity >= 0.8)

use opsis_core::event::{StateEvent, WorldDelta};
use tokio::sync::broadcast;
use tracing::warn;

/// Threshold above which an event is also sent to the fast-path channel.
const FAST_PATH_SEVERITY: f32 = 0.8;

/// Central event bus for the Opsis engine.
///
/// Wraps three broadcast channels so that producers (`publish_*`) and consumers
/// (`subscribe_*`) are fully decoupled.
#[derive(Debug)]
pub struct EventBus {
    event_tx: broadcast::Sender<StateEvent>,
    delta_tx: broadcast::Sender<WorldDelta>,
    fast_path_tx: broadcast::Sender<StateEvent>,
}

impl EventBus {
    /// Create a new bus with default channel capacities.
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(16_384);
        let (delta_tx, _) = broadcast::channel(256);
        let (fast_path_tx, _) = broadcast::channel(1_024);
        Self {
            event_tx,
            delta_tx,
            fast_path_tx,
        }
    }

    // ── Publish ──────────────────────────────────────────────────────

    /// Send a state event to all subscribers.
    ///
    /// If `severity >= 0.8`, the event is also sent to the fast-path channel.
    pub fn publish_event(&self, event: StateEvent) {
        if event.severity >= FAST_PATH_SEVERITY && self.fast_path_tx.send(event.clone()).is_err() {
            warn!("fast_path_tx has no receivers");
        }
        let _ = self.event_tx.send(event);
    }

    /// Send a world delta to all subscribers.
    pub fn publish_delta(&self, delta: WorldDelta) {
        if let Err(e) = self.delta_tx.send(delta) {
            warn!("delta_tx has no receivers: {e}");
        }
    }

    // ── Subscribe ────────────────────────────────────────────────────

    /// Subscribe to all state events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<StateEvent> {
        self.event_tx.subscribe()
    }

    /// Subscribe to world deltas (one per tick).
    pub fn subscribe_deltas(&self) -> broadcast::Receiver<WorldDelta> {
        self.delta_tx.subscribe()
    }

    /// Subscribe to high-severity events only.
    pub fn subscribe_fast_path(&self) -> broadcast::Receiver<StateEvent> {
        self.fast_path_tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opsis_core::clock::WorldTick;
    use opsis_core::event::EventId;
    use opsis_core::feed::FeedSource;
    use opsis_core::state::StateDomain;

    /// Helper to build a state event with a given severity.
    fn make_event(severity: f32) -> StateEvent {
        StateEvent {
            id: EventId::default(),
            tick: WorldTick(1),
            domain: StateDomain::Emergency,
            location: None,
            severity,
            summary: format!("test event severity={severity}"),
            source: FeedSource::new("test"),
            tags: vec![],
            raw_ref: EventId::default(),
        }
    }

    #[tokio::test]
    async fn event_bus_delivers_to_subscriber() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe_events();

        let event = make_event(0.5);
        bus.publish_event(event.clone());

        let received = rx.recv().await.expect("should receive event");
        assert_eq!(received.id, event.id);
    }

    #[tokio::test]
    async fn high_severity_goes_to_fast_path() {
        let bus = EventBus::new();
        let mut fast_rx = bus.subscribe_fast_path();
        let mut all_rx = bus.subscribe_events();

        let event = make_event(0.9);
        bus.publish_event(event.clone());

        // Should appear on both channels.
        let fast = fast_rx.recv().await.expect("should receive on fast path");
        let all = all_rx.recv().await.expect("should receive on events");
        assert_eq!(fast.id, event.id);
        assert_eq!(all.id, event.id);
    }

    #[tokio::test]
    async fn low_severity_skips_fast_path() {
        let bus = EventBus::new();
        let mut fast_rx = bus.subscribe_fast_path();
        let mut all_rx = bus.subscribe_events();

        let event = make_event(0.3);
        bus.publish_event(event.clone());

        // Should appear on events but NOT on fast path.
        let all = all_rx.recv().await.expect("should receive on events");
        assert_eq!(all.id, event.id);

        // fast_path should be empty — try_recv should fail.
        assert!(fast_rx.try_recv().is_err());
    }
}
