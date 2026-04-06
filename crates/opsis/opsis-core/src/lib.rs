//! `opsis-core` — Core types and traits for the Opsis world state engine.
//!
//! This crate contains **zero IO** — only types, traits, and pure logic.

pub mod clock;
pub mod error;
pub mod event;
pub mod feed;
pub mod spatial;
pub mod state;
pub mod subscription;

// Re-export key types for convenience.
pub use clock::{WorldClock, WorldTick};
pub use error::{OpsisError, OpsisResult};
pub use event::{EventId, RawFeedEvent, StateEvent, StateLineDelta, WorldDelta};
pub use feed::{FeedIngestor, FeedSource, SchemaKey};
pub use spatial::{Bbox, GeoHotspot, GeoPoint};
pub use state::{StateDomain, StateLine, Trend, WorldState};
pub use subscription::{ClientId, Subscription};
