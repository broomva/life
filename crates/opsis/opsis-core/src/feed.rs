use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::OpsisResult;
use crate::event::{OpsisEvent, RawFeedEvent};

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

impl fmt::Display for SchemaKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ── Feed connector configuration (declarative, from feeds.toml) ─────

/// Transport configuration for a feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "connector")]
pub enum ConnectorConfig {
    #[serde(rename = "poll")]
    Poll {
        url: String,
        /// Poll interval in seconds.
        interval_secs: u64,
    },
    #[serde(rename = "websocket")]
    WebSocket { url: String },
    #[serde(rename = "rss")]
    Rss {
        url: String,
        /// Poll interval in seconds.
        interval_secs: u64,
    },
    #[serde(rename = "agent_stream")]
    AgentStream { url: String },
}

/// Authentication configuration for a feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub user_env: Option<String>,
    pub pass_env: Option<String>,
    pub token_env: Option<String>,
}

/// Declarative feed definition (from feeds.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedConfig {
    pub name: String,
    #[serde(flatten)]
    pub connector: ConnectorConfig,
    pub schema: String,
    pub domain: Option<String>,
    pub auth: Option<AuthConfig>,
}

/// Top-level feeds.toml structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedsConfig {
    #[serde(default)]
    pub feeds: Vec<FeedConfig>,
}

// ── Feed ingestor trait ─────────────────────────────────────────────

/// Trait for components that ingest data from an external feed and normalise
/// it into [`OpsisEvent`]s.
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

    /// Normalise a single raw event into zero or more opsis events.
    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<OpsisEvent>>;

    /// How often this feed should be polled.
    fn poll_interval(&self) -> Duration;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_source_display() {
        let src = FeedSource::new("usgs-earthquake");
        assert_eq!(src.to_string(), "usgs-earthquake");
    }

    #[test]
    fn schema_key_display() {
        let key = SchemaKey::new("usgs.geojson.v1");
        assert_eq!(key.to_string(), "usgs.geojson.v1");
    }

    #[test]
    fn feeds_config_toml_roundtrip() {
        let toml_str = r#"
[[feeds]]
name = "usgs-earthquake"
connector = "poll"
url = "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/all_hour.geojson"
interval_secs = 30
schema = "usgs.geojson.v1"
domain = "Emergency"

[[feeds]]
name = "open-meteo"
connector = "poll"
url = "https://api.open-meteo.com/v1/forecast"
interval_secs = 300
schema = "openmeteo.current.v1"
domain = "Weather"
"#;
        let config: FeedsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.feeds.len(), 2);
        assert_eq!(config.feeds[0].name, "usgs-earthquake");
        assert_eq!(config.feeds[1].name, "open-meteo");

        match &config.feeds[0].connector {
            ConnectorConfig::Poll { url, interval_secs } => {
                assert!(url.contains("usgs.gov"));
                assert_eq!(*interval_secs, 30);
            }
            other => panic!("expected Poll, got {:?}", other),
        }
    }

    #[test]
    fn connector_config_websocket_variant() {
        let toml_str = r#"
[[feeds]]
name = "live-stream"
connector = "websocket"
url = "wss://example.com/ws"
schema = "ws.v1"
"#;
        let config: FeedsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.feeds.len(), 1);
        match &config.feeds[0].connector {
            ConnectorConfig::WebSocket { url } => {
                assert_eq!(url, "wss://example.com/ws");
            }
            other => panic!("expected WebSocket, got {:?}", other),
        }
    }

    #[test]
    fn connector_config_agent_stream_variant() {
        let toml_str = r#"
[[feeds]]
name = "arcan-agent-0"
connector = "agent_stream"
url = "http://localhost:3000"
schema = "arcan.agent.v1"
"#;
        let config: FeedsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.feeds.len(), 1);
        assert_eq!(config.feeds[0].name, "arcan-agent-0");
        match &config.feeds[0].connector {
            ConnectorConfig::AgentStream { url } => {
                assert_eq!(url, "http://localhost:3000");
            }
            other => panic!("expected AgentStream, got {:?}", other),
        }
    }

    #[test]
    fn feed_config_with_auth() {
        let toml_str = r#"
[[feeds]]
name = "authed-feed"
connector = "poll"
url = "https://api.example.com/data"
interval_secs = 60
schema = "example.v1"

[feeds.auth]
type = "bearer"
token_env = "EXAMPLE_API_TOKEN"
"#;
        let config: FeedsConfig = toml::from_str(toml_str).unwrap();
        let feed = &config.feeds[0];
        let auth = feed.auth.as_ref().unwrap();
        assert_eq!(auth.auth_type, "bearer");
        assert_eq!(auth.token_env.as_deref(), Some("EXAMPLE_API_TOKEN"));
    }
}
