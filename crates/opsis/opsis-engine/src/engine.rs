//! Main Opsis engine — tick loop, feed management, shutdown.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, broadcast};
use tokio::time;
use tracing::{info, warn};

use opsis_core::clock::WorldClock;
use opsis_core::event::{OpsisEvent, WorldDelta};
use opsis_core::feed::FeedIngestor;
use opsis_core::state::WorldState;

use crate::aggregator::TickAggregator;
use crate::bus::EventBus;
use crate::gaia::GaiaAnalyzer;

/// A serializable snapshot of the world state for new clients.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorldSnapshot {
    /// Current world state (domains, activities, trends).
    pub world_state: WorldState,
    /// Most recent WorldDelta (last tick).
    pub last_delta: Option<WorldDelta>,
    /// Recent events across all domains (last 200).
    pub recent_events: Vec<OpsisEvent>,
    /// Recent Gaia insights (last 20).
    pub recent_gaia_insights: Vec<OpsisEvent>,
}

const MAX_SNAPSHOT_EVENTS: usize = 200;
const MAX_SNAPSHOT_GAIA: usize = 20;

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

/// Shared handle for reading the current world snapshot.
pub type SnapshotHandle = Arc<RwLock<Option<WorldSnapshot>>>;

/// Optional callback to persist events to an external store (e.g. opsis-lago).
///
/// Receives a batch of OpsisEvents produced each tick. The implementation
/// must be non-blocking (return immediately, queue internally).
pub type EventPersistFn = Box<dyn Fn(&[OpsisEvent]) + Send + Sync>;

/// The main Opsis world state engine.
pub struct OpsisEngine {
    pub config: EngineConfig,
    pub bus: Arc<EventBus>,
    /// Shared snapshot updated every tick — readable by HTTP handlers.
    pub snapshot: SnapshotHandle,
    world: WorldState,
    aggregator: TickAggregator,
    gaia: GaiaAnalyzer,
    feeds: Vec<Box<dyn FeedIngestor>>,
    /// Ring buffer of recent events for the snapshot.
    recent_events: Vec<OpsisEvent>,
    /// Ring buffer of recent Gaia insights for the snapshot.
    recent_gaia: Vec<OpsisEvent>,
    /// Optional persistence callback (e.g. opsis-lago writer).
    persist_fn: Option<EventPersistFn>,
}

impl OpsisEngine {
    /// Create a new engine with the given configuration.
    pub fn new(config: EngineConfig) -> Self {
        let clock = WorldClock::new(config.hz);
        Self {
            config,
            bus: Arc::new(EventBus::new()),
            snapshot: Arc::new(RwLock::new(None)),
            world: WorldState::new(clock),
            aggregator: TickAggregator::new(),
            gaia: GaiaAnalyzer::new(),
            feeds: Vec::new(),
            recent_events: Vec::new(),
            recent_gaia: Vec::new(),
            persist_fn: None,
        }
    }

    /// Set a persistence callback for event durability (e.g. opsis-lago).
    pub fn set_persist_fn(&mut self, f: EventPersistFn) {
        self.persist_fn = Some(f);
    }

    /// Inject replayed events into the engine's state (for startup replay).
    pub fn replay_events(&mut self, events: Vec<OpsisEvent>) {
        let count = events.len();
        for event in events {
            self.aggregator.push(event);
        }
        // Flush all replayed events into world state.
        let _delta = self.aggregator.flush(&mut self.world);
        info!(
            replayed = count,
            "replayed persisted events into world state"
        );
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
        let snapshot_handle = self.snapshot.clone();

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

                    // Run Gaia analysis post-flush.
                    let gaia_insights = self.gaia.analyze(&self.world, &delta);
                    if !gaia_insights.is_empty() {
                        info!(
                            tick = %self.world.clock.tick,
                            count = gaia_insights.len(),
                            "gaia insights generated"
                        );
                    }

                    // Bundle Gaia insights into the delta before broadcast.
                    let delta = WorldDelta {
                        gaia_insights,
                        ..delta
                    };

                    // Accumulate recent events for the snapshot.
                    let mut tick_events: Vec<OpsisEvent> = Vec::new();
                    for sld in &delta.state_line_deltas {
                        tick_events.extend(sld.new_events.iter().cloned());
                    }
                    tick_events.extend(delta.unrouted_events.iter().cloned());
                    tick_events.extend(delta.gaia_insights.iter().cloned());

                    // Persist events to Lago (non-blocking).
                    if let Some(ref persist) = self.persist_fn
                        && !tick_events.is_empty()
                    {
                        persist(&tick_events);
                    }

                    self.recent_events.extend(tick_events);
                    if self.recent_events.len() > MAX_SNAPSHOT_EVENTS {
                        let drain = self.recent_events.len() - MAX_SNAPSHOT_EVENTS;
                        self.recent_events.drain(..drain);
                    }
                    self.recent_gaia.extend(delta.gaia_insights.iter().cloned());
                    if self.recent_gaia.len() > MAX_SNAPSHOT_GAIA {
                        let drain = self.recent_gaia.len() - MAX_SNAPSHOT_GAIA;
                        self.recent_gaia.drain(..drain);
                    }

                    // Update shared snapshot (non-blocking write).
                    {
                        let snap = WorldSnapshot {
                            world_state: self.world.clone(),
                            last_delta: Some(delta.clone()),
                            recent_events: self.recent_events.clone(),
                            recent_gaia_insights: self.recent_gaia.clone(),
                        };
                        *snapshot_handle.write().await = Some(snap);
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
