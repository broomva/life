//! NASA FIRMS (Fire Information for Resource Management System) feed.
//!
//! Returns active fire hotspots worldwide with precise coordinates,
//! brightness, confidence, and fire radiative power.
//!
//! Requires `FIRMS_MAP_KEY` environment variable (free registration).
//! API docs: https://firms.modaps.eosdis.nasa.gov/api/area

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use opsis_core::error::{OpsisError, OpsisResult};
use opsis_core::event::{EventId, EventSource, OpsisEvent, OpsisEventKind, RawFeedEvent};
use opsis_core::feed::{FeedIngestor, FeedSource, SchemaKey};
use opsis_core::spatial::GeoPoint;
use opsis_core::state::StateDomain;

/// NASA FIRMS active fire hotspot feed.
pub struct FirmsFeed {
    client: reqwest::Client,
    map_key: Option<String>,
}

impl Default for FirmsFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl FirmsFeed {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            map_key: std::env::var("FIRMS_MAP_KEY").ok(),
        }
    }

    /// Map fire radiative power (MW) to severity (0.0–1.0).
    fn frp_to_severity(frp: f64) -> f32 {
        // FRP typically ranges 0–500+ MW. 100 MW is a significant fire.
        (frp / 200.0).clamp(0.0, 1.0) as f32
    }
}

impl FeedIngestor for FirmsFeed {
    fn source(&self) -> FeedSource {
        FeedSource::new("nasa-firms")
    }

    fn schema(&self) -> SchemaKey {
        SchemaKey::new("nasa.firms.v1")
    }

    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>> {
        let has_key = self.map_key.is_some();
        Box::pin(async move {
            if has_key {
                tracing::info!("NASA FIRMS feed ready (MAP_KEY configured)");
                Ok(())
            } else {
                tracing::warn!("NASA FIRMS feed disabled — set FIRMS_MAP_KEY env var to enable");
                Err(OpsisError::Feed("FIRMS_MAP_KEY not set".into()))
            }
        })
    }

    fn poll_raw(
        &self,
    ) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>> {
        let map_key = self.map_key.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let map_key =
                map_key.ok_or_else(|| OpsisError::Feed("FIRMS_MAP_KEY not set".into()))?;

            // FIRMS CSV endpoint: VIIRS_SNPP_NRT, world, last 1 day
            let url = format!(
                "https://firms.modaps.eosdis.nasa.gov/api/area/csv/{map_key}/VIIRS_SNPP_NRT/world/1"
            );

            let resp = client
                .get(&url)
                .send()
                .await
                .map_err(|e| OpsisError::Feed(format!("firms: {e}")))?;

            let csv_text = resp
                .text()
                .await
                .map_err(|e| OpsisError::Feed(format!("firms: {e}")))?;

            // Parse CSV: header line + data lines
            let mut lines = csv_text.lines();
            let header = lines.next().unwrap_or("");
            let columns: Vec<&str> = header.split(',').collect();

            // Find column indices
            let lat_idx = columns.iter().position(|c| *c == "latitude");
            let lon_idx = columns.iter().position(|c| *c == "longitude");
            let bright_idx = columns.iter().position(|c| *c == "bright_ti4");
            let conf_idx = columns.iter().position(|c| *c == "confidence");
            let frp_idx = columns.iter().position(|c| *c == "frp");

            let (lat_i, lon_i) = match (lat_idx, lon_idx) {
                (Some(la), Some(lo)) => (la, lo),
                _ => return Err(OpsisError::Feed("firms: missing lat/lon columns".into())),
            };

            let mut events = Vec::new();
            for line in lines.take(1000) {
                // Limit to 1000 fires per poll
                let fields: Vec<&str> = line.split(',').collect();
                if fields.len() <= lat_i.max(lon_i) {
                    continue;
                }

                let lat: f64 = fields[lat_i].parse().unwrap_or(0.0);
                let lon: f64 = fields[lon_i].parse().unwrap_or(0.0);
                let frp: f64 = frp_idx
                    .and_then(|i| fields.get(i))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0);
                let brightness: f64 = bright_idx
                    .and_then(|i| fields.get(i))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0);
                let confidence = conf_idx
                    .and_then(|i| fields.get(i))
                    .map(|v| v.to_string())
                    .unwrap_or_default();

                events.push(RawFeedEvent {
                    id: EventId::default(),
                    timestamp: Utc::now(),
                    source: FeedSource::new("nasa-firms"),
                    feed_schema: SchemaKey::new("nasa.firms.v1"),
                    location: Some(GeoPoint::new(lat, lon)),
                    payload: serde_json::json!({
                        "frp": frp,
                        "brightness": brightness,
                        "confidence": confidence,
                    }),
                });
            }

            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<OpsisEvent>> {
        let frp = raw.payload["frp"].as_f64().unwrap_or(0.0);
        let brightness = raw.payload["brightness"].as_f64().unwrap_or(0.0);
        let confidence = raw.payload["confidence"].as_str().unwrap_or("low");

        let summary =
            format!("Active fire — {frp:.1} MW, brightness {brightness:.0}, conf: {confidence}");

        Ok(vec![OpsisEvent {
            id: raw.id.clone(),
            tick: opsis_core::clock::WorldTick::zero(),
            timestamp: raw.timestamp,
            source: EventSource::Feed(raw.source.clone()),
            kind: OpsisEventKind::WorldObservation { summary },
            location: raw.location,
            domain: Some(StateDomain::Emergency),
            severity: Some(Self::frp_to_severity(frp)),
            schema_key: self.schema(),
            tags: vec!["fire".into(), "firms".into(), "wildfire".into()],
        }])
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(600) // 10 minutes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_fire_event() {
        let feed = FirmsFeed::new();
        let raw = RawFeedEvent {
            id: EventId::default(),
            timestamp: Utc::now(),
            source: FeedSource::new("nasa-firms"),
            feed_schema: SchemaKey::new("nasa.firms.v1"),
            location: Some(GeoPoint::new(-15.5, 28.3)),
            payload: json!({"frp": 85.5, "brightness": 342.1, "confidence": "high"}),
        };

        let events = feed.normalize(&raw).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        match &event.kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert!(summary.contains("85.5 MW"));
                assert!(summary.contains("high"));
            }
            _ => panic!("expected WorldObservation"),
        }
        assert_eq!(event.domain, Some(StateDomain::Emergency));
        // 85.5 / 200 ≈ 0.4275
        assert!((event.severity.unwrap() - 0.4275).abs() < 0.01);
    }

    #[test]
    fn frp_to_severity_range() {
        assert!((FirmsFeed::frp_to_severity(0.0) - 0.0).abs() < f32::EPSILON);
        assert!((FirmsFeed::frp_to_severity(100.0) - 0.5).abs() < f32::EPSILON);
        assert!((FirmsFeed::frp_to_severity(200.0) - 1.0).abs() < f32::EPSILON);
        assert!((FirmsFeed::frp_to_severity(500.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn source_and_schema() {
        let feed = FirmsFeed::new();
        assert_eq!(feed.source().to_string(), "nasa-firms");
        assert_eq!(feed.schema().to_string(), "nasa.firms.v1");
    }
}
