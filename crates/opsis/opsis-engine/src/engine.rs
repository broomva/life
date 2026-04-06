//! Main Opsis engine — tick loop, feed management, shutdown.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::time;
use tracing::{info, warn};

use opsis_core::clock::WorldClock;
use opsis_core::feed::FeedIngestor;
use opsis_core::state::WorldState;

use crate::aggregator::TickAggregator;
use crate::bus::EventBus;

/// Configuration for the Opsis engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Tick rate in Hz (default: 1.0).
    pub hz: f64,
    /// Server bind address (default: 127.0.0.1:3010).
    pub bind_addr: String,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            hz: 1.0,
            bind_addr: "127.0.0.1:3010".into(),
        }
    }
}

/// The main Opsis world state engine.
pub struct OpsisEngine {
    pub config: EngineConfig,
    pub bus: Arc<EventBus>,
    world: WorldState,
    aggregator: TickAggregator,
    feeds: Vec<Box<dyn FeedIngestor>>,
}

impl OpsisEngine {
    /// Create a new engine with the given configuration.
    pub fn new(config: EngineConfig) -> Self {
        let clock = WorldClock::new(config.hz);
        Self {
            config,
            bus: Arc::new(EventBus::new()),
            world: WorldState::new(clock),
            aggregator: TickAggregator::new(),
            feeds: Vec::new(),
        }
    }

    /// Register a data feed ingestor.
    pub fn add_feed(&mut self, feed: Box<dyn FeedIngestor>) {
        self.feeds.push(feed);
    }

    /// Run the engine. Spawns feed tasks and the 1 Hz tick loop.
    /// Returns when the shutdown signal is received.
    pub async fn run(mut self, mut shutdown: broadcast::Receiver<()>) {
        info!(
            hz = self.config.hz,
            feeds = self.feeds.len(),
            "opsis engine starting"
        );

        let bus = self.bus.clone();

        // Subscribe to event bus BEFORE spawning feeds to avoid missing early events.
        let mut event_rx = bus.subscribe_events();

        // Spawn one task per feed.
        let mut feed_handles = Vec::new();
        for feed in self.feeds.drain(..) {
            let bus = bus.clone();
            let handle = tokio::spawn(async move {
                if let Err(e) = feed.connect().await {
                    warn!(source = %feed.source(), "feed connect failed: {e}");
                    return;
                }
                info!(source = %feed.source(), "feed connected");

                loop {
                    match feed.poll_raw().await {
                        Ok(raw_events) => {
                            let raw_count = raw_events.len();
                            let mut published = 0u32;
                            for raw in &raw_events {
                                match feed.normalize(raw) {
                                    Ok(state_events) => {
                                        for event in state_events {
                                            bus.publish_event(event);
                                            published += 1;
                                        }
                                    }
                                    Err(e) => {
                                        warn!(source = %feed.source(), "normalize error: {e}");
                                    }
                                }
                            }
                            if raw_count > 0 {
                                info!(
                                    source = %feed.source(),
                                    raw = raw_count,
                                    published = published,
                                    "feed poll complete"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(source = %feed.source(), "poll error: {e}");
                        }
                    }
                    tokio::time::sleep(feed.poll_interval()).await;
                }
            });
            feed_handles.push(handle);
        }

        // Tick loop.
        let tick_duration = Duration::from_secs_f64(1.0 / self.config.hz);
        let mut tick_interval = time::interval(tick_duration);

        loop {
            tokio::select! {
                _ = tick_interval.tick() => {
                    // Drain pending events into the aggregator.
                    let mut drained = 0u32;
                    loop {
                        match event_rx.try_recv() {
                            Ok(event) => {
                                self.aggregator.push(event);
                                drained += 1;
                            }
                            Err(broadcast::error::TryRecvError::Empty) => break,
                            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                                warn!(lagged = n, "event bus lagged — events dropped");
                                break;
                            }
                            Err(broadcast::error::TryRecvError::Closed) => return,
                        }
                    }

                    // Advance clock and flush aggregator.
                    self.world.clock.advance();
                    let delta = self.aggregator.flush(&mut self.world);

                    if drained > 0 || !delta.state_line_deltas.is_empty() {
                        info!(
                            tick = %self.world.clock.tick,
                            drained,
                            deltas = delta.state_line_deltas.len(),
                            "tick flush"
                        );
                    }

                    // Always broadcast — UI needs tick updates even when no events.
                    bus.publish_delta(delta);
                }
                _ = shutdown.recv() => {
                    info!("opsis engine shutting down");
                    for handle in feed_handles {
                        handle.abort();
                    }
                    return;
                }
            }
        }
    }
}
