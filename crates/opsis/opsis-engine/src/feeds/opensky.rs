//! OpenSky Network flight tracking feed.
//!
//! Polls the OpenSky REST API for live aircraft state vectors.
//! The API returns an array-of-arrays format (not JSON objects),
//! so this requires a custom ingestor rather than GenericPollFeed.
//!
//! API docs: https://openskynetwork.github.io/opensky-api/rest.html
//! Rate limit: anonymous 10s, authenticated 5s.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use opsis_core::error::{OpsisError, OpsisResult};
use opsis_core::event::{EventId, EventSource, OpsisEvent, OpsisEventKind, RawFeedEvent};
use opsis_core::feed::{FeedIngestor, FeedSource, SchemaKey};
use opsis_core::spatial::GeoPoint;
use opsis_core::state::StateDomain;

/// OpenSky states endpoint (global, no bounding box).
const OPENSKY_URL: &str = "https://opensky-network.org/api/states/all";

/// OpenSky flight tracking feed — polls state vectors for live aircraft.
pub struct OpenSkyFeed {
    client: reqwest::Client,
    /// The URL to poll (may include bbox query params from feeds.toml).
    pub(crate) url: String,
    /// Poll interval in seconds.
    interval_secs: u64,
}

impl Default for OpenSkyFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenSkyFeed {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            url: OPENSKY_URL.into(),
            interval_secs: 15,
        }
    }

    /// Create with a custom URL and interval (from feeds.toml config).
    pub fn with_config(url: String, interval_secs: u64) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
            interval_secs,
        }
    }

    /// Map altitude (meters) to severity (0.0–1.0). Higher = less "severe".
    /// Ground aircraft = 0, cruising at 12km+ = 1.0.
    fn altitude_to_severity(alt_meters: f64) -> f32 {
        (alt_meters / 12000.0).clamp(0.0, 1.0) as f32
    }
}

impl FeedIngestor for OpenSkyFeed {
    fn source(&self) -> FeedSource {
        FeedSource::new("opensky-flights")
    }

    fn schema(&self) -> SchemaKey {
        SchemaKey::new("opensky.states.v1")
    }

    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn poll_raw(
        &self,
    ) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>> {
        let url = self.url.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let resp = client
                .get(&url)
                .send()
                .await
                .map_err(|e| OpsisError::Feed(format!("opensky: {e}")))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| OpsisError::Feed(format!("opensky: {e}")))?;

            let empty = Vec::new();
            let states = body["states"].as_array().unwrap_or(&empty);
            let mut events = Vec::new();

            for state in states {
                let arr = match state.as_array() {
                    Some(a) if a.len() >= 13 => a,
                    _ => continue,
                };

                // OpenSky state vector indices:
                // 0: icao24, 1: callsign, 2: origin_country
                // 5: longitude, 6: latitude, 7: baro_altitude
                // 8: on_ground, 9: velocity, 10: true_track
                let lon = arr[5].as_f64();
                let lat = arr[6].as_f64();
                let alt = arr[7].as_f64().unwrap_or(0.0);

                let location = match (lat, lon) {
                    (Some(lat), Some(lon)) => Some(GeoPoint::new(lat, lon)),
                    _ => continue, // Skip aircraft without position
                };

                let callsign = arr[1].as_str().unwrap_or("unknown").trim().to_string();
                let country = arr[2].as_str().unwrap_or("unknown");
                let velocity_kts = arr[9].as_f64().unwrap_or(0.0) * 1.94384; // m/s → knots

                events.push(RawFeedEvent {
                    id: EventId::default(),
                    timestamp: Utc::now(),
                    source: FeedSource::new("opensky-flights"),
                    feed_schema: SchemaKey::new("opensky.states.v1"),
                    location,
                    payload: serde_json::json!({
                        "icao24": arr[0],
                        "callsign": callsign,
                        "origin_country": country,
                        "altitude_m": alt,
                        "velocity_kts": velocity_kts,
                        "on_ground": arr[8],
                        "true_track": arr[10],
                    }),
                });
            }

            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<OpsisEvent>> {
        let callsign = raw.payload["callsign"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        let country = raw.payload["origin_country"].as_str().unwrap_or("unknown");
        let alt = raw.payload["altitude_m"].as_f64().unwrap_or(0.0);
        let velocity = raw.payload["velocity_kts"].as_f64().unwrap_or(0.0);

        let summary = if callsign.is_empty() || callsign == "unknown" {
            format!("Aircraft from {country} at {alt:.0}m")
        } else {
            format!("{callsign} ({country}) — {alt:.0}m, {velocity:.0}kts")
        };

        Ok(vec![OpsisEvent {
            id: raw.id.clone(),
            tick: opsis_core::clock::WorldTick::zero(),
            timestamp: raw.timestamp,
            source: EventSource::Feed(raw.source.clone()),
            kind: OpsisEventKind::WorldObservation { summary },
            location: raw.location,
            domain: Some(StateDomain::Infrastructure),
            severity: Some(Self::altitude_to_severity(alt)),
            schema_key: self.schema(),
            tags: vec!["opensky".into(), "flight".into(), "aviation".into()],
        }])
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.interval_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_flight_event() {
        let feed = OpenSkyFeed::new();
        let raw = RawFeedEvent {
            id: EventId::default(),
            timestamp: Utc::now(),
            source: FeedSource::new("opensky-flights"),
            feed_schema: SchemaKey::new("opensky.states.v1"),
            location: Some(GeoPoint::new(40.7128, -74.006)),
            payload: json!({
                "icao24": "abc123",
                "callsign": "UAL454",
                "origin_country": "United States",
                "altitude_m": 10972.8,
                "velocity_kts": 324.5,
                "on_ground": false,
                "true_track": 248.1,
            }),
        };

        let events = feed.normalize(&raw).unwrap();
        assert_eq!(events.len(), 1);

        let event = &events[0];
        match &event.kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert!(summary.contains("UAL454"));
                assert!(summary.contains("United States"));
                assert!(summary.contains("10973m"));
            }
            _ => panic!("expected WorldObservation"),
        }
        assert_eq!(event.domain, Some(StateDomain::Infrastructure));
        // 10972.8 / 12000 ≈ 0.914
        assert!((event.severity.unwrap() - 0.914).abs() < 0.01);
        assert!(event.location.is_some());
        assert_eq!(event.tags, vec!["opensky", "flight", "aviation"]);
    }

    #[test]
    fn altitude_to_severity_range() {
        assert!((OpenSkyFeed::altitude_to_severity(0.0) - 0.0).abs() < f32::EPSILON);
        assert!((OpenSkyFeed::altitude_to_severity(6000.0) - 0.5).abs() < f32::EPSILON);
        assert!((OpenSkyFeed::altitude_to_severity(12000.0) - 1.0).abs() < f32::EPSILON);
        assert!((OpenSkyFeed::altitude_to_severity(15000.0) - 1.0).abs() < f32::EPSILON); // clamped
    }

    #[test]
    fn config_with_custom_url_and_interval() {
        let url =
            "https://opensky-network.org/api/states/all?lamin=25&lomin=-130&lamax=50&lomax=-60";
        let feed = OpenSkyFeed::with_config(url.into(), 30);
        assert!(feed.url.contains("lamin=25"));
        assert!(feed.url.contains("lomin=-130"));
        assert_eq!(feed.poll_interval(), Duration::from_secs(30));
    }

    #[test]
    fn source_and_schema() {
        let feed = OpenSkyFeed::new();
        assert_eq!(feed.source().to_string(), "opensky-flights");
        assert_eq!(feed.schema().to_string(), "opensky.states.v1");
    }
}
