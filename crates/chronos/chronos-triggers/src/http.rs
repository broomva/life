//! [`HttpTrigger`] — the real HTTP-backed wake trigger (M1).
//!
//! Replaces `HttpTriggerStub`. The trigger is deliberately thin: it owns the **receiving** end of
//! an mpsc channel and yields whatever [`WakeEvent`]s are pushed into it. The **sending** end is
//! handed to the `chronos-api` HTTP server, which constructs a wake on each `POST /v1/wake` whose
//! intent is ready to fire now.
//!
//! This split keeps the dependency rule clean: `chronos-triggers` depends only on `chronos-core`
//! (the channel carries `chronos_core::WakeEvent`). `chronos-api` never depends on
//! `chronos-triggers` — it holds a bare `mpsc::Sender<WakeEvent>` instead — and `chronosd` is the
//! single place that wires the two ends together via [`wake_channel`].

use async_trait::async_trait;
use chronos_core::{WakeEvent, WakeTrigger};
use tokio::sync::mpsc;

/// Sender half of the wake-ingest channel, handed to the HTTP API layer. Pushing a [`WakeEvent`]
/// here causes the paired [`HttpTrigger`] to yield it on its next `next_wake`.
pub type WakeSender = mpsc::Sender<WakeEvent>;

/// Real HTTP-backed wake trigger. Yields events pushed into the paired [`WakeSender`].
///
/// `next_wake` returns `None` once **every** sender has been dropped (i.e. the API server has shut
/// down), at which point the [`chronos_core::WakeRouter`] drops the trigger cleanly.
pub struct HttpTrigger {
    rx: mpsc::Receiver<WakeEvent>,
}

impl HttpTrigger {
    /// Construct a trigger from the receiving end of a wake-ingest channel. Most callers want
    /// [`wake_channel`] instead, which builds the matched pair.
    pub fn new(rx: mpsc::Receiver<WakeEvent>) -> Self {
        Self { rx }
    }
}

/// Build a matched ([`WakeSender`], [`HttpTrigger`]) pair with the given channel capacity.
///
/// The daemon registers the returned [`HttpTrigger`] with the [`chronos_core::WakeRouter`] and
/// hands the [`WakeSender`] to `chronos-api`. `buffer` is clamped to at least 1.
pub fn wake_channel(buffer: usize) -> (WakeSender, HttpTrigger) {
    let (tx, rx) = mpsc::channel(buffer.max(1));
    (tx, HttpTrigger::new(rx))
}

#[async_trait]
impl WakeTrigger for HttpTrigger {
    async fn next_wake(&mut self) -> Option<WakeEvent> {
        self.rx.recv().await
    }

    fn name(&self) -> &'static str {
        "http"
    }
}

#[cfg(test)]
mod tests {
    use chronos_core::{WakeEvent, WakeSource, WakeTrigger};

    use super::wake_channel;

    /// Build an HTTP wake the way `chronos-api` does, without reaching across crates.
    fn http_intent(intent: impl Into<String>) -> WakeEvent {
        WakeEvent::new(WakeSource::Http)
            .with_payload(serde_json::json!({ "intent": intent.into() }))
    }

    #[tokio::test]
    async fn pushed_event_is_yielded() {
        let (tx, mut trigger) = wake_channel(8);
        let sent = http_intent("rebuild index");
        tx.send(sent.clone()).await.expect("send");

        let got = trigger.next_wake().await.expect("trigger yields the event");
        assert_eq!(got.source, WakeSource::Http);
        assert_eq!(got.event_id, sent.event_id);
        assert_eq!(got.payload["intent"], "rebuild index");
    }

    #[tokio::test]
    async fn events_are_yielded_fifo() {
        let (tx, mut trigger) = wake_channel(8);
        for n in 0..3 {
            tx.send(http_intent(format!("intent-{n}")))
                .await
                .expect("send");
        }
        for n in 0..3 {
            let got = trigger.next_wake().await.expect("event");
            assert_eq!(got.payload["intent"], format!("intent-{n}"));
        }
    }

    #[tokio::test]
    async fn dropping_all_senders_exhausts_the_trigger() {
        let (tx, mut trigger) = wake_channel(4);
        drop(tx);
        assert!(
            trigger.next_wake().await.is_none(),
            "trigger should return None once every sender is dropped"
        );
    }

    #[tokio::test]
    async fn name_is_stable() {
        let (_tx, trigger) = wake_channel(1);
        assert_eq!(trigger.name(), "http");
    }
}
