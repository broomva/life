//! [`HeartbeatTrigger`] — the one real trigger in M0.

use std::time::Duration;

use async_trait::async_trait;
use chronos_core::{WakeEvent, WakeSource, WakeTrigger};

/// Periodic timer-driven trigger.
///
/// Sleeps for [`HeartbeatTrigger::interval`] then emits a `WakeSource::Heartbeat` event,
/// repeating until dropped (or the router closes its receiver). Used as the simplest possible
/// wake source to exercise the full Chronos pipeline (trigger → router → lago journal) before
/// real triggers come online.
///
/// ## Choosing an interval
///
/// - **Dev**: 5–10 seconds is convenient for human-pace debugging.
/// - **Production**: 60 seconds (or more) avoids journal-noise for the system session.
/// - **Avoid**: ≤ 1 second — floods the journal and consumes redb compaction budget.
pub struct HeartbeatTrigger {
    /// How long between heartbeat fires.
    pub interval: Duration,
}

impl HeartbeatTrigger {
    /// Construct a heartbeat trigger with the supplied interval.
    pub fn new(interval: Duration) -> Self {
        Self { interval }
    }
}

#[async_trait]
impl WakeTrigger for HeartbeatTrigger {
    async fn next_wake(&mut self) -> Option<WakeEvent> {
        tokio::time::sleep(self.interval).await;
        let payload = serde_json::json!({
            "interval_ms": self.interval.as_millis() as u64,
        });
        Some(WakeEvent::new(WakeSource::Heartbeat).with_payload(payload))
    }

    fn name(&self) -> &'static str {
        "heartbeat"
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chronos_core::{WakeSource, WakeTrigger};

    use super::HeartbeatTrigger;

    #[tokio::test]
    async fn heartbeat_emits_at_configured_interval() {
        let mut h = HeartbeatTrigger::new(Duration::from_millis(20));
        let start = std::time::Instant::now();
        let mut events = Vec::with_capacity(3);
        for _ in 0..3 {
            let e = h.next_wake().await.expect("heartbeat emits");
            events.push(e);
        }
        let elapsed = start.elapsed();

        // Each tick takes >= 20ms; 3 ticks take >= 60ms. Allow up to 1s of slack for slow CI.
        assert!(
            elapsed >= Duration::from_millis(60),
            "heartbeat fired too quickly: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "heartbeat fired too slowly: {elapsed:?}"
        );

        for e in &events {
            assert_eq!(e.source, WakeSource::Heartbeat);
            assert_eq!(e.payload["interval_ms"], 20);
            assert!(e.fired_at_unix_ms > 0);
        }
        // Each event id should be unique.
        assert_ne!(events[0].event_id, events[1].event_id);
        assert_ne!(events[1].event_id, events[2].event_id);
    }

    #[tokio::test]
    async fn heartbeat_trigger_name_is_stable() {
        let h = HeartbeatTrigger::new(Duration::from_millis(10));
        assert_eq!(h.name(), "heartbeat");
    }
}
