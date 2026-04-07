//! AISStream.io ship tracking feed (WebSocket).
//!
//! Connects to AISStream.io WebSocket API for real-time AIS position reports.
//! Requires `AISSTREAM_API_KEY` environment variable.
//!
//! API docs: https://aisstream.io/documentation

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use opsis_core::error::{OpsisError, OpsisResult};
use opsis_core::event::{EventId, EventSource, OpsisEvent, OpsisEventKind, RawFeedEvent};
use opsis_core::feed::{FeedIngestor, FeedSource, SchemaKey};
use opsis_core::spatial::GeoPoint;
use opsis_core::state::StateDomain;

const AISSTREAM_WS_URL: &str = "wss://stream.aisstream.io/v0/stream";

/// AIS ship tracking feed.
///
/// Since the WebSocket connector isn't wired into the engine's feed loop yet,
/// this uses a poll-based approach: connect, subscribe, read N messages, disconnect.
/// Each poll cycle collects a batch of position reports.
pub struct AisStreamFeed {
    api_key: Option<String>,
    /// Bounding box: [[lat_min, lon_min], [lat_max, lon_max]]
    bbox: [[f64; 2]; 2],
    /// How many messages to collect per poll cycle.
    batch_size: usize,
}

impl Default for AisStreamFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl AisStreamFeed {
    pub fn new() -> Self {
        Self {
            api_key: std::env::var("AISSTREAM_API_KEY").ok(),
            bbox: [[-90.0, -180.0], [90.0, 180.0]], // Global
            batch_size: 100,
        }
    }

    /// Map speed over ground (knots) to severity (0.0–1.0).
    /// Stationary = 0, fast vessel (30+ kts) = 1.0.
    fn speed_to_severity(sog_knots: f64) -> f32 {
        (sog_knots / 30.0).clamp(0.0, 1.0) as f32
    }
}

impl FeedIngestor for AisStreamFeed {
    fn source(&self) -> FeedSource {
        FeedSource::new("ais-ships")
    }

    fn schema(&self) -> SchemaKey {
        SchemaKey::new("aisstream.position.v1")
    }

    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>> {
        let has_key = self.api_key.is_some();
        Box::pin(async move {
            if has_key {
                tracing::info!("AISStream feed ready (API key configured)");
                Ok(())
            } else {
                tracing::warn!("AISStream feed disabled — set AISSTREAM_API_KEY env var to enable");
                Err(OpsisError::Feed("AISSTREAM_API_KEY not set".into()))
            }
        })
    }

    fn poll_raw(
        &self,
    ) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>> {
        let api_key = self.api_key.clone();
        let bbox = self.bbox;
        let batch_size = self.batch_size;

        Box::pin(async move {
            let api_key =
                api_key.ok_or_else(|| OpsisError::Feed("AISSTREAM_API_KEY not set".into()))?;

            use futures_util::{SinkExt, StreamExt};
            use tokio_tungstenite::connect_async;

            let (mut ws, _) = connect_async(AISSTREAM_WS_URL)
                .await
                .map_err(|e| OpsisError::Feed(format!("ais websocket connect: {e}")))?;

            // Send subscription message within 3 seconds.
            let subscribe = serde_json::json!({
                "APIKey": api_key,
                "BoundingBoxes": [bbox],
                "FilterMessageTypes": ["PositionReport"],
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                subscribe.to_string().into(),
            ))
            .await
            .map_err(|e| OpsisError::Feed(format!("ais subscribe: {e}")))?;

            // Collect up to batch_size messages with a timeout.
            let mut events = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

            loop {
                if events.len() >= batch_size {
                    break;
                }

                let msg = tokio::select! {
                    msg = ws.next() => msg,
                    _ = tokio::time::sleep_until(deadline) => break,
                };

                let msg = match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => text,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "ais websocket read error");
                        break;
                    }
                    None => break,
                };

                let parsed: serde_json::Value = match serde_json::from_str(&msg) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Extract position from metadata (more reliable than Message.PositionReport).
                let meta = &parsed["MetaData"];
                let lat = meta["latitude"].as_f64();
                let lon = meta["longitude"].as_f64();
                let ship_name = meta["ShipName"]
                    .as_str()
                    .unwrap_or("unknown")
                    .trim()
                    .to_string();
                let mmsi = meta["MMSI"].as_u64().unwrap_or(0);

                let location = match (lat, lon) {
                    (Some(lat), Some(lon)) => Some(GeoPoint::new(lat, lon)),
                    _ => continue,
                };

                // Extract speed from the message.
                let pos_report = &parsed["Message"]["PositionReport"];
                let sog = pos_report["Sog"].as_f64().unwrap_or(0.0);
                let cog = pos_report["Cog"].as_f64().unwrap_or(0.0);
                let heading = pos_report["TrueHeading"].as_f64();

                events.push(RawFeedEvent {
                    id: EventId::default(),
                    timestamp: Utc::now(),
                    source: FeedSource::new("ais-ships"),
                    feed_schema: SchemaKey::new("aisstream.position.v1"),
                    location,
                    payload: serde_json::json!({
                        "mmsi": mmsi,
                        "ship_name": ship_name,
                        "sog_kts": sog,
                        "cog": cog,
                        "heading": heading,
                    }),
                });
            }

            // Close WebSocket cleanly.
            let _ = ws.close(None).await;

            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<OpsisEvent>> {
        let ship_name = raw.payload["ship_name"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        let sog = raw.payload["sog_kts"].as_f64().unwrap_or(0.0);
        let cog = raw.payload["cog"].as_f64().unwrap_or(0.0);
        let mmsi = raw.payload["mmsi"].as_u64().unwrap_or(0);

        let summary = if ship_name.is_empty() || ship_name == "unknown" {
            format!("Ship MMSI:{mmsi} — {sog:.1}kts, COG {cog:.0}°")
        } else {
            format!("{ship_name} — {sog:.1}kts, COG {cog:.0}°")
        };

        Ok(vec![OpsisEvent {
            id: raw.id.clone(),
            tick: opsis_core::clock::WorldTick::zero(),
            timestamp: raw.timestamp,
            source: EventSource::Feed(raw.source.clone()),
            kind: OpsisEventKind::WorldObservation { summary },
            location: raw.location,
            domain: Some(StateDomain::Ocean),
            severity: Some(Self::speed_to_severity(sog)),
            schema_key: self.schema(),
            tags: vec!["ais".into(), "ship".into(), "maritime".into()],
        }])
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_ship_event() {
        let feed = AisStreamFeed::new();
        let raw = RawFeedEvent {
            id: EventId::default(),
            timestamp: Utc::now(),
            source: FeedSource::new("ais-ships"),
            feed_schema: SchemaKey::new("aisstream.position.v1"),
            location: Some(GeoPoint::new(51.9, 4.5)),
            payload: json!({
                "mmsi": 244630000u64,
                "ship_name": "STENA BRITANNICA",
                "sog_kts": 18.5,
                "cog": 135.0,
                "heading": 133.0,
            }),
        };

        let events = feed.normalize(&raw).unwrap();
        assert_eq!(events.len(), 1);

        let event = &events[0];
        match &event.kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert!(summary.contains("STENA BRITANNICA"));
                assert!(summary.contains("18.5kts"));
            }
            _ => panic!("expected WorldObservation"),
        }
        assert_eq!(event.domain, Some(StateDomain::Ocean));
        // 18.5 / 30 ≈ 0.617
        assert!((event.severity.unwrap() - 0.617).abs() < 0.01);
        assert_eq!(event.tags, vec!["ais", "ship", "maritime"]);
    }

    #[test]
    fn speed_to_severity_range() {
        assert!((AisStreamFeed::speed_to_severity(0.0) - 0.0).abs() < f32::EPSILON);
        assert!((AisStreamFeed::speed_to_severity(15.0) - 0.5).abs() < f32::EPSILON);
        assert!((AisStreamFeed::speed_to_severity(30.0) - 1.0).abs() < f32::EPSILON);
        assert!((AisStreamFeed::speed_to_severity(50.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn source_and_schema() {
        let feed = AisStreamFeed::new();
        assert_eq!(feed.source().to_string(), "ais-ships");
        assert_eq!(feed.schema().to_string(), "aisstream.position.v1");
    }
}
