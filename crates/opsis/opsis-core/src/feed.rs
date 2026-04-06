use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::OpsisResult;
use crate::event::{RawFeedEvent, StateEvent};

/// Identifies a feed source (e.g. "gdelt", "usgs-earthquakes").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FeedSource(pub String);

impl FeedSource {
    /// Create a new feed source identifier.
    pub fn new(name: &str) -> Self {
        Self(name.to_owned())
    }
}

impl fmt::Display for FeedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies the schema/format of events from a feed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemaKey(pub String);

impl SchemaKey {
    /// Create a new schema key.
    pub fn new(name: &str) -> Self {
        Self(name.to_owned())
    }
}

/// Trait for components that ingest data from an external feed and normalise
/// it into [`StateEvent`]s.
///
/// Uses `Pin<Box<dyn Future>>` return types for dyn-compatibility (the
/// workspace runs Rust 2024 which supports `async fn` in traits, but
/// dyn-dispatch still requires boxing).
pub trait FeedIngestor: Send + Sync {
    /// The source identifier for this feed.
    fn source(&self) -> FeedSource;

    /// The schema key describing the events this feed produces.
    fn schema(&self) -> SchemaKey;

    /// Establish a connection to the upstream feed.
    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>>;

    /// Poll the upstream feed for new raw events.
    fn poll_raw(&self)
    -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>>;

    /// Normalise a single raw event into zero or more state events.
    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<StateEvent>>;

    /// How often this feed should be polled.
    fn poll_interval(&self) -> Duration;
}
