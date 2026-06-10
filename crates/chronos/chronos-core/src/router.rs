//! [`WakeRouter`] — multiplexes concurrent triggers into a single wake stream.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::{WakeEvent, WakeTrigger};

/// Multiplexes multiple [`WakeTrigger`] sources into a single async stream of [`WakeEvent`]s.
///
/// Each trigger runs in its own tokio task and pushes events into a shared mpsc channel.
/// The daemon's main loop consumes via [`WakeRouter::next_wake`] and dispatches downstream.
///
/// ## Buffer capacity
///
/// The internal mpsc channel is bounded. If a trigger produces faster than the daemon
/// drains, the trigger's `tx.send` will await — backpressure flows naturally into the
/// trigger's own loop. Pick a capacity large enough that bursts (e.g. fs watch storms) are
/// absorbed, small enough that runaway triggers can't OOM the daemon.
pub struct WakeRouter {
    rx: mpsc::Receiver<WakeEvent>,
    tx: mpsc::Sender<WakeEvent>,
    handles: Vec<JoinHandle<()>>,
}

impl WakeRouter {
    /// Construct a new router with the supplied buffer capacity. 64 is a reasonable default
    /// for dev; production deployments with many concurrent triggers may want more.
    pub fn new(buffer: usize) -> Self {
        let buffer = buffer.max(1);
        let (tx, rx) = mpsc::channel(buffer);
        Self {
            rx,
            tx,
            handles: Vec::new(),
        }
    }

    /// Spawn a trigger as a tokio task that forwards its events into the router.
    ///
    /// The task lives until the trigger returns `None` from [`WakeTrigger::next_wake`] or
    /// the router is dropped (which closes the receiver and breaks the send loop).
    pub fn add_trigger(&mut self, mut trigger: Box<dyn WakeTrigger>) {
        let tx = self.tx.clone();
        let name = trigger.name();
        let handle = tokio::spawn(async move {
            info!(trigger = name, "trigger started");
            while let Some(event) = trigger.next_wake().await {
                if tx.send(event).await.is_err() {
                    warn!(trigger = name, "router closed; trigger forwarder exiting");
                    break;
                }
            }
            info!(trigger = name, "trigger exhausted");
        });
        self.handles.push(handle);
    }

    /// Block until the next wake event arrives, or all triggers have exhausted.
    ///
    /// Returns `None` once every spawned trigger has returned `None` AND the router's
    /// internal sender has been dropped (which happens in [`WakeRouter::shutdown`]).
    pub async fn next_wake(&mut self) -> Option<WakeEvent> {
        self.rx.recv().await
    }

    /// Number of trigger forwarder tasks currently spawned.
    pub fn trigger_count(&self) -> usize {
        self.handles.len()
    }

    /// Drop the router's own sender and abort all forwarder tasks. After this returns,
    /// [`WakeRouter::next_wake`] drains the buffer then returns `None` permanently.
    pub async fn shutdown(self) {
        // Drop self.tx first so the receiver can EOF cleanly after the buffer drains.
        let WakeRouter {
            mut rx,
            tx,
            handles,
        } = self;
        drop(tx);
        for handle in &handles {
            handle.abort();
        }
        for handle in handles {
            // Ignore join errors from aborted tasks.
            let _ = handle.await;
        }
        rx.close();
        while rx.recv().await.is_some() {}
    }
}

impl Default for WakeRouter {
    fn default() -> Self {
        Self::new(64)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{WakeEvent, WakeSource, WakeTrigger};

    /// Test trigger that emits N events then returns None.
    struct CountingTrigger {
        remaining: usize,
        name: &'static str,
    }

    #[async_trait::async_trait]
    impl WakeTrigger for CountingTrigger {
        async fn next_wake(&mut self) -> Option<WakeEvent> {
            if self.remaining == 0 {
                return None;
            }
            self.remaining -= 1;
            // tiny yield so events from concurrent triggers interleave deterministically.
            tokio::task::yield_now().await;
            Some(WakeEvent::new(WakeSource::Heartbeat))
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[tokio::test]
    async fn router_drains_a_single_trigger() {
        let mut router = WakeRouter::new(8);
        router.add_trigger(Box::new(CountingTrigger {
            remaining: 3,
            name: "test",
        }));

        let mut count = 0;
        // Wait at most 1 second total for events to flow through.
        let deadline = tokio::time::sleep(Duration::from_secs(1));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                evt = router.next_wake() => match evt {
                    Some(_) => count += 1,
                    None => break,
                },
                _ = &mut deadline => panic!("router stalled before draining 3 events"),
            }
            if count == 3 {
                break;
            }
        }
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn router_multiplexes_two_triggers() {
        let mut router = WakeRouter::new(16);
        router.add_trigger(Box::new(CountingTrigger {
            remaining: 5,
            name: "a",
        }));
        router.add_trigger(Box::new(CountingTrigger {
            remaining: 5,
            name: "b",
        }));
        assert_eq!(router.trigger_count(), 2);

        let mut count = 0;
        let deadline = tokio::time::sleep(Duration::from_secs(1));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                evt = router.next_wake() => match evt {
                    Some(_) => count += 1,
                    None => break,
                },
                _ = &mut deadline => panic!("router stalled before draining 10 events"),
            }
            if count == 10 {
                break;
            }
        }
        assert_eq!(count, 10);
    }

    #[tokio::test]
    async fn router_shutdown_drains_and_returns_none() {
        let mut router = WakeRouter::new(4);
        router.add_trigger(Box::new(CountingTrigger {
            remaining: 2,
            name: "shutdown-test",
        }));
        let _ = router.next_wake().await.expect("first event");
        router.shutdown().await;
    }
}
