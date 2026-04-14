//! Feed factory — builds `Box<dyn FeedIngestor>` from `FeedConfig`.
//!
//! Resolution order:
//! 1. Config has `normalize` section → GenericPollFeed (no Rust code needed)
//! 2. Name matches a registered custom feed → use that implementation
//! 3. Otherwise → error with helpful message

use opsis_core::feed::{ConnectorConfig, FeedConfig, FeedIngestor};

use crate::error::{EngineError, EngineResult};
use crate::feeds::ais::AisStreamFeed;
use crate::feeds::firms::FirmsFeed;
use crate::feeds::generic_poll::GenericPollFeed;
use crate::feeds::ioda::IodaFeed;
use crate::feeds::opensky::OpenSkyFeed;
use crate::feeds::polymarket::PolymarketFeed;
use crate::feeds::usgs::UsgsEarthquakeFeed;
use crate::feeds::weather::OpenMeteoWeatherFeed;

/// Build a `FeedIngestor` from a declarative `FeedConfig`.
///
/// If the config has a `[feeds.normalize]` section, a `GenericPollFeed` is used.
/// Otherwise, falls back to known custom implementations by name.
pub fn build_feed(config: &FeedConfig) -> EngineResult<Box<dyn FeedIngestor>> {
    // Generic feed: config has a normalizer section.
    if let Some(ref normalizer) = config.normalize {
        return match &config.connector {
            ConnectorConfig::Poll { .. } | ConnectorConfig::Rss { .. } => Ok(Box::new(
                GenericPollFeed::new(config.clone(), normalizer.clone()),
            )),
            ConnectorConfig::AgentStream { .. } => Err(EngineError::Config(
                "agent_stream feeds use inject-mode, not GenericPollFeed".into(),
            )),
            ConnectorConfig::WebSocket { .. } => Err(EngineError::Config(
                "WebSocket connector with generic normalizer not yet supported".into(),
            )),
        };
    }

    // Custom feed: match by name.
    match config.name.as_str() {
        "usgs-earthquake" => Ok(Box::new(UsgsEarthquakeFeed::new())),
        "open-meteo" => Ok(Box::new(OpenMeteoWeatherFeed::new())),
        name if name.starts_with("opensky") => {
            let (url, interval) = match &config.connector {
                ConnectorConfig::Poll { url, interval_secs } => (url.clone(), *interval_secs),
                _ => ("https://opensky-network.org/api/states/all".into(), 15),
            };
            Ok(Box::new(OpenSkyFeed::with_config(url, interval)))
        }
        "ais-ships" => Ok(Box::new(AisStreamFeed::new())),
        "nasa-firms" => Ok(Box::new(FirmsFeed::new())),
        "ioda-outages" => Ok(Box::new(IodaFeed::new())),
        "polymarket" => Ok(Box::new(PolymarketFeed::new())),
        _ => Err(EngineError::Config(format!(
            "unknown feed '{}' — add a [feeds.normalize] section for generic feeds \
             or register a custom FeedIngestor",
            config.name
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opsis_core::feed::{ConnectorConfig, FeedConfig, NormalizerConfig};

    fn poll_config(name: &str, normalize: Option<NormalizerConfig>) -> FeedConfig {
        FeedConfig {
            name: name.into(),
            connector: ConnectorConfig::Poll {
                url: "http://example.com".into(),
                interval_secs: 60,
            },
            schema: "test.v1".into(),
            domain: None,
            auth: None,
            normalize,
        }
    }

    fn simple_normalizer() -> NormalizerConfig {
        NormalizerConfig {
            events_path: None,
            summary: "$.title".into(),
            severity: None,
            severity_range: [0.0, 1.0],
            lat: None,
            lon: None,
            tags: vec![],
        }
    }

    #[test]
    fn build_usgs_by_name() {
        let feed = build_feed(&poll_config("usgs-earthquake", None));
        assert!(feed.is_ok());
        assert_eq!(feed.unwrap().source().to_string(), "usgs-earthquake");
    }

    #[test]
    fn build_open_meteo_by_name() {
        let feed = build_feed(&poll_config("open-meteo", None));
        assert!(feed.is_ok());
        assert_eq!(feed.unwrap().source().to_string(), "open-meteo");
    }

    #[test]
    fn build_generic_with_normalizer() {
        let feed = build_feed(&poll_config("gdelt-events", Some(simple_normalizer())));
        assert!(feed.is_ok());
        assert_eq!(feed.unwrap().source().to_string(), "gdelt-events");
    }

    #[test]
    fn build_unknown_without_normalizer_fails() {
        let result = build_feed(&poll_config("unknown-feed", None));
        assert!(result.is_err());
        match result {
            Err(e) => {
                let msg = e.to_string();
                assert!(msg.contains("unknown feed"), "got: {msg}");
            }
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn build_agent_stream_with_normalizer_fails() {
        let config = FeedConfig {
            name: "agent".into(),
            connector: ConnectorConfig::AgentStream {
                url: "http://localhost:3000".into(),
            },
            schema: "arcan.agent.v1".into(),
            domain: None,
            auth: None,
            normalize: Some(simple_normalizer()),
        };
        assert!(build_feed(&config).is_err());
    }
}
