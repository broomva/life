//! Generic poll-based feed — implements FeedIngestor using declarative NormalizerConfig.
//!
//! Adding a new JSON feed = TOML entry with `[feeds.normalize]` section. No Rust code.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use opsis_core::error::{OpsisError, OpsisResult};
use opsis_core::event::{EventId, EventSource, OpsisEvent, OpsisEventKind, RawFeedEvent};
use opsis_core::feed::{
    ConnectorConfig, FeedConfig, FeedIngestor, FeedSource, NormalizerConfig, SchemaKey,
};
use opsis_core::spatial::GeoPoint;
use opsis_core::state::StateDomain;

use crate::jsonpath;

/// A generic poll-based feed driven entirely by `FeedConfig` + `NormalizerConfig`.
pub struct GenericPollFeed {
    config: FeedConfig,
    normalizer: NormalizerConfig,
    domain: Option<StateDomain>,
    client: reqwest::Client,
}

impl GenericPollFeed {
    pub fn new(config: FeedConfig, normalizer: NormalizerConfig) -> Self {
        let domain = config.domain.as_deref().map(StateDomain::from_name);
        Self {
            config,
            normalizer,
            domain,
            client: reqwest::Client::new(),
        }
    }

    fn url(&self) -> &str {
        match &self.config.connector {
            ConnectorConfig::Poll { url, .. } => url,
            ConnectorConfig::Rss { url, .. } => url,
            _ => "",
        }
    }

    fn interval_secs(&self) -> u64 {
        match &self.config.connector {
            ConnectorConfig::Poll { interval_secs, .. } => *interval_secs,
            ConnectorConfig::Rss { interval_secs, .. } => *interval_secs,
            _ => 60,
        }
    }

    /// Normalize a raw severity value to 0.0–1.0 using the configured range.
    fn normalize_severity(&self, raw: f64) -> f32 {
        let [min, max] = self.normalizer.severity_range;
        if (max - min).abs() < f64::EPSILON {
            return 0.5;
        }
        ((raw - min) / (max - min)).clamp(0.0, 1.0) as f32
    }
}

impl FeedIngestor for GenericPollFeed {
    fn source(&self) -> FeedSource {
        FeedSource::new(&self.config.name)
    }

    fn schema(&self) -> SchemaKey {
        SchemaKey::new(&self.config.schema)
    }

    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>> {
        let name = self.config.name.clone();
        let url = self.url().to_string();
        Box::pin(async move {
            tracing::info!(feed = %name, url = %url, "GenericPollFeed connected");
            Ok(())
        })
    }

    fn poll_raw(
        &self,
    ) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>> {
        let url = self.url().to_string();
        let client = self.client.clone();
        let source = self.source();
        let schema = self.schema();
        let events_path = self.normalizer.events_path.clone();
        let lat_path = self.normalizer.lat.clone();
        let lon_path = self.normalizer.lon.clone();

        // Resolve auth
        let auth = self.config.auth.clone();

        Box::pin(async move {
            let mut request = client.get(&url);

            // Apply auth
            if let Some(ref auth_cfg) = auth {
                match auth_cfg.auth_type.as_str() {
                    "bearer" => {
                        if let Some(ref env_key) = auth_cfg.token_env
                            && let Ok(token) = std::env::var(env_key)
                        {
                            request = request.bearer_auth(token);
                        }
                    }
                    "basic" => {
                        let user = auth_cfg
                            .user_env
                            .as_deref()
                            .and_then(|k| std::env::var(k).ok())
                            .unwrap_or_default();
                        let pass = auth_cfg
                            .pass_env
                            .as_deref()
                            .and_then(|k| std::env::var(k).ok());
                        request = request.basic_auth(user, pass);
                    }
                    _ => {}
                }
            }

            let resp = request
                .send()
                .await
                .map_err(|e| OpsisError::Feed(format!("HTTP error: {e}")))?;
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| OpsisError::Feed(format!("JSON parse error: {e}")))?;

            // Extract array of events (or treat whole response as single event).
            let items = match &events_path {
                Some(path) => jsonpath::extract_array(&body, path),
                None => vec![body],
            };

            let mut events = Vec::with_capacity(items.len());
            for item in items {
                // Extract location if configured.
                let location = match (&lat_path, &lon_path) {
                    (Some(lat_p), Some(lon_p)) => {
                        let lat = jsonpath::extract_f64(&item, lat_p);
                        let lon = jsonpath::extract_f64(&item, lon_p);
                        match (lat, lon) {
                            (Some(lat), Some(lon)) => Some(GeoPoint::new(lat, lon)),
                            _ => None,
                        }
                    }
                    _ => None,
                };

                events.push(RawFeedEvent {
                    id: EventId::default(),
                    timestamp: chrono::Utc::now(),
                    source: source.clone(),
                    feed_schema: schema.clone(),
                    location,
                    payload: item,
                });
            }

            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<OpsisEvent>> {
        // Extract summary.
        let summary = jsonpath::extract_str(&raw.payload, &self.normalizer.summary)
            .unwrap_or_else(|| "event".to_string());

        // Extract and normalize severity.
        let severity = self
            .normalizer
            .severity
            .as_deref()
            .and_then(|path| jsonpath::extract_f64(&raw.payload, path))
            .map(|raw_sev| self.normalize_severity(raw_sev));

        let event = OpsisEvent {
            id: raw.id.clone(),
            tick: opsis_core::clock::WorldTick::zero(),
            timestamp: raw.timestamp,
            source: EventSource::Feed(raw.source.clone()),
            kind: OpsisEventKind::WorldObservation { summary },
            location: raw.location,
            domain: self.domain.clone(),
            severity,
            schema_key: self.schema(),
            tags: self.normalizer.tags.clone(),
        };

        Ok(vec![event])
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.interval_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_config(normalizer: NormalizerConfig) -> FeedConfig {
        FeedConfig {
            name: "test-feed".into(),
            connector: ConnectorConfig::Poll {
                url: "http://example.com/api".into(),
                interval_secs: 60,
            },
            schema: "test.v1".into(),
            domain: Some("Finance".into()),
            auth: None,
            normalize: Some(normalizer),
        }
    }

    fn make_normalizer() -> NormalizerConfig {
        NormalizerConfig {
            events_path: None,
            summary: "$.title".into(),
            severity: Some("$.score".into()),
            severity_range: [0.0, 10.0],
            lat: Some("$.lat".into()),
            lon: Some("$.lon".into()),
            tags: vec!["test".into()],
        }
    }

    #[test]
    fn source_and_schema() {
        let feed = GenericPollFeed::new(make_config(make_normalizer()), make_normalizer());
        assert_eq!(feed.source().to_string(), "test-feed");
        assert_eq!(feed.schema().to_string(), "test.v1");
    }

    #[test]
    fn poll_interval_from_config() {
        let feed = GenericPollFeed::new(make_config(make_normalizer()), make_normalizer());
        assert_eq!(feed.poll_interval(), Duration::from_secs(60));
    }

    #[test]
    fn normalize_basic() {
        let feed = GenericPollFeed::new(make_config(make_normalizer()), make_normalizer());
        let raw = RawFeedEvent {
            id: EventId::default(),
            timestamp: chrono::Utc::now(),
            source: FeedSource::new("test-feed"),
            feed_schema: SchemaKey::new("test.v1"),
            location: Some(GeoPoint::new(4.0, -74.0)),
            payload: json!({"title": "Market crash", "score": 8.5, "lat": 4.0, "lon": -74.0}),
        };

        let events = feed.normalize(&raw).unwrap();
        assert_eq!(events.len(), 1);

        let event = &events[0];
        match &event.kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert_eq!(summary, "Market crash");
            }
            _ => panic!("expected WorldObservation"),
        }
        assert_eq!(event.severity, Some(0.85)); // 8.5 / 10.0
        assert_eq!(event.domain, Some(StateDomain::Finance));
        assert_eq!(event.tags, vec!["test"]);
    }

    #[test]
    fn normalize_severity_clamped() {
        let feed = GenericPollFeed::new(make_config(make_normalizer()), make_normalizer());
        assert!((feed.normalize_severity(12.0) - 1.0).abs() < f32::EPSILON); // above max → 1.0
        assert!((feed.normalize_severity(-2.0) - 0.0).abs() < f32::EPSILON); // below min → 0.0
    }

    #[test]
    fn normalize_goldstein_range() {
        let mut norm = make_normalizer();
        norm.severity_range = [-10.0, 10.0];
        let feed = GenericPollFeed::new(make_config(norm.clone()), norm);
        assert!((feed.normalize_severity(5.0) - 0.75).abs() < f32::EPSILON); // (5 - -10) / 20 = 0.75
        assert!((feed.normalize_severity(-10.0) - 0.0).abs() < f32::EPSILON);
        assert!((feed.normalize_severity(10.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn normalize_missing_severity() {
        let mut norm = make_normalizer();
        norm.severity = None;
        let feed = GenericPollFeed::new(make_config(norm.clone()), norm);
        let raw = RawFeedEvent {
            id: EventId::default(),
            timestamp: chrono::Utc::now(),
            source: FeedSource::new("test"),
            feed_schema: SchemaKey::new("test.v1"),
            location: None,
            payload: json!({"title": "No severity"}),
        };
        let events = feed.normalize(&raw).unwrap();
        assert!(events[0].severity.is_none());
    }

    #[test]
    fn normalize_missing_summary_defaults() {
        let feed = GenericPollFeed::new(make_config(make_normalizer()), make_normalizer());
        let raw = RawFeedEvent {
            id: EventId::default(),
            timestamp: chrono::Utc::now(),
            source: FeedSource::new("test"),
            feed_schema: SchemaKey::new("test.v1"),
            location: None,
            payload: json!({"no_title_field": true}),
        };
        let events = feed.normalize(&raw).unwrap();
        match &events[0].kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert_eq!(summary, "event"); // default
            }
            _ => panic!("expected WorldObservation"),
        }
    }
}
