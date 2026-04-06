//! USGS earthquake real-time feed ingestor.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use chrono::Utc;
use opsis_core::OpsisError;
use opsis_core::clock::WorldTick;
use opsis_core::error::OpsisResult;
use opsis_core::event::{EventId, RawFeedEvent, StateEvent};
use opsis_core::feed::{FeedIngestor, FeedSource, SchemaKey};
use opsis_core::spatial::GeoPoint;
use opsis_core::state::StateDomain;

const USGS_URL: &str = "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/all_hour.geojson";

/// USGS earthquake feed — polls hourly GeoJSON summary, deduplicates by ID.
pub struct UsgsEarthquakeFeed {
    client: reqwest::Client,
    seen_ids: Mutex<HashSet<String>>,
}

impl Default for UsgsEarthquakeFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl UsgsEarthquakeFeed {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            seen_ids: Mutex::new(HashSet::new()),
        }
    }

    /// Map earthquake magnitude (0–10) to severity (0.0–1.0).
    pub fn magnitude_to_severity(mag: f64) -> f32 {
        (mag / 8.0).clamp(0.0, 1.0) as f32
    }
}

impl FeedIngestor for UsgsEarthquakeFeed {
    fn source(&self) -> FeedSource {
        FeedSource::new("usgs-earthquake")
    }

    fn schema(&self) -> SchemaKey {
        SchemaKey::new("usgs.geojson.v1")
    }

    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn poll_raw(
        &self,
    ) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>> {
        Box::pin(async {
            let resp = self
                .client
                .get(USGS_URL)
                .send()
                .await
                .map_err(|e| OpsisError::Feed(format!("usgs: {e}")))?;

            let geojson: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| OpsisError::Feed(format!("usgs: {e}")))?;

            let empty = Vec::new();
            let features = geojson["features"].as_array().unwrap_or(&empty);
            let mut events = Vec::new();

            let mut seen = self.seen_ids.lock().unwrap();

            for feature in features {
                let usgs_id = feature["id"].as_str().unwrap_or_default().to_string();
                if seen.contains(&usgs_id) {
                    continue;
                }
                seen.insert(usgs_id);

                let coords = &feature["geometry"]["coordinates"];
                let lon = coords[0].as_f64().unwrap_or(0.0);
                let lat = coords[1].as_f64().unwrap_or(0.0);

                events.push(RawFeedEvent {
                    id: EventId::default(),
                    timestamp: Utc::now(),
                    source: self.source(),
                    feed_schema: self.schema(),
                    location: Some(GeoPoint::new(lat, lon)),
                    payload: feature.clone(),
                });
            }

            // Prevent unbounded growth.
            if seen.len() > 500 {
                seen.clear();
            }

            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<StateEvent>> {
        let props = &raw.payload["properties"];
        let mag = props["mag"].as_f64().unwrap_or(0.0);
        let place = props["place"]
            .as_str()
            .unwrap_or("Unknown location")
            .to_string();

        Ok(vec![StateEvent {
            id: EventId::default(),
            tick: WorldTick::zero(), // Engine sets actual tick.
            domain: StateDomain::Emergency,
            location: raw.location,
            severity: Self::magnitude_to_severity(mag),
            summary: format!("M{mag:.1} earthquake — {place}"),
            source: raw.source.clone(),
            tags: vec!["earthquake".into(), "seismic".into()],
            raw_ref: raw.id.clone(),
        }])
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnitude_to_severity_scaling() {
        assert!(UsgsEarthquakeFeed::magnitude_to_severity(0.0) < 0.01);
        assert!(UsgsEarthquakeFeed::magnitude_to_severity(4.0) > 0.3);
        assert!(UsgsEarthquakeFeed::magnitude_to_severity(7.0) > 0.7);
        assert!((UsgsEarthquakeFeed::magnitude_to_severity(10.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn normalize_usgs_event() {
        let feed = UsgsEarthquakeFeed::new();
        let raw = RawFeedEvent {
            id: EventId("test-raw".into()),
            timestamp: Utc::now(),
            source: feed.source(),
            feed_schema: feed.schema(),
            location: Some(GeoPoint::new(3.4, -76.5)),
            payload: serde_json::json!({
                "properties": {
                    "mag": 6.2,
                    "place": "10km SE of Cali, Colombia"
                }
            }),
        };
        let events = feed.normalize(&raw).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].severity > 0.7);
        assert!(events[0].summary.contains("6.2"));
        assert!(events[0].summary.contains("Cali"));
    }
}
