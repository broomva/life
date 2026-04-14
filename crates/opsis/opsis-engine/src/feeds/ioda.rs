//! IODA (Internet Outage Detection and Analysis) feed.
//!
//! Country-level internet connectivity alerts. Free, no auth.
//! High-signal for Gaia cross-domain analysis — outages correlate with
//! conflict, natural disasters, and political events.
//!
//! API docs: https://api.ioda.inetintel.cc.gatech.edu/v2/

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use opsis_core::error::{OpsisError, OpsisResult};
use opsis_core::event::{EventId, EventSource, OpsisEvent, OpsisEventKind, RawFeedEvent};
use opsis_core::feed::{FeedIngestor, FeedSource, SchemaKey};
use opsis_core::spatial::GeoPoint;
use opsis_core::state::StateDomain;

/// IODA internet outage feed.
pub struct IodaFeed {
    client: reqwest::Client,
}

impl Default for IodaFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl IodaFeed {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl FeedIngestor for IodaFeed {
    fn source(&self) -> FeedSource {
        FeedSource::new("ioda-outages")
    }

    fn schema(&self) -> SchemaKey {
        SchemaKey::new("ioda.outages.v1")
    }

    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn poll_raw(
        &self,
    ) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>> {
        let client = self.client.clone();

        Box::pin(async move {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let from = now - 3600; // Last hour

            let url = format!(
                "https://api.ioda.inetintel.cc.gatech.edu/v2/outages/alerts?from={from}&until={now}&limit=50&entityType=country"
            );

            let resp = client
                .get(&url)
                .send()
                .await
                .map_err(|e| OpsisError::Feed(format!("ioda: {e}")))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| OpsisError::Feed(format!("ioda: {e}")))?;

            let empty = Vec::new();
            let alerts = body["data"].as_array().unwrap_or(&empty);

            let mut events = Vec::new();
            for alert in alerts {
                let entity = &alert["entity"];
                let code = entity["code"].as_str().unwrap_or("??");
                let name = entity["name"].as_str().unwrap_or("Unknown");
                let level = alert["level"].as_str().unwrap_or("normal");

                // Map country code to approximate centroid
                let location = COUNTRY_CENTROIDS
                    .get(code)
                    .map(|(lat, lon)| GeoPoint::new(*lat, *lon));

                events.push(RawFeedEvent {
                    id: EventId::default(),
                    timestamp: Utc::now(),
                    source: FeedSource::new("ioda-outages"),
                    feed_schema: SchemaKey::new("ioda.outages.v1"),
                    location,
                    payload: serde_json::json!({
                        "country_code": code,
                        "country_name": name,
                        "level": level,
                        "value": alert["value"],
                        "history_value": alert["historyValue"],
                    }),
                });
            }

            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<OpsisEvent>> {
        let country = raw.payload["country_name"].as_str().unwrap_or("Unknown");
        let code = raw.payload["country_code"].as_str().unwrap_or("??");
        let level = raw.payload["level"].as_str().unwrap_or("normal");

        let severity = match level {
            "critical" => 0.9,
            "warning" => 0.6,
            _ => 0.2,
        };

        let summary = format!("{country} ({code}) — internet {level}");

        Ok(vec![OpsisEvent {
            id: raw.id.clone(),
            tick: opsis_core::clock::WorldTick::zero(),
            timestamp: raw.timestamp,
            source: EventSource::Feed(raw.source.clone()),
            kind: OpsisEventKind::WorldObservation { summary },
            location: raw.location,
            domain: Some(StateDomain::Infrastructure),
            severity: Some(severity),
            schema_key: self.schema(),
            tags: vec!["ioda".into(), "internet".into(), "outage".into()],
        }])
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(300) // 5 minutes
    }
}

/// Approximate country centroids for geo-pinning IODA alerts.
static COUNTRY_CENTROIDS: std::sync::LazyLock<HashMap<&'static str, (f64, f64)>> =
    std::sync::LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert("US", (39.8, -98.6));
        m.insert("GB", (54.0, -2.0));
        m.insert("DE", (51.2, 10.4));
        m.insert("FR", (46.2, 2.2));
        m.insert("CN", (35.9, 104.2));
        m.insert("RU", (61.5, 105.3));
        m.insert("IN", (20.6, 79.0));
        m.insert("BR", (-14.2, -51.9));
        m.insert("JP", (36.2, 138.3));
        m.insert("AU", (-25.3, 133.8));
        m.insert("CA", (56.1, -106.3));
        m.insert("MX", (23.6, -102.6));
        m.insert("ZA", (-30.6, 22.9));
        m.insert("NG", (9.1, 8.7));
        m.insert("EG", (26.8, 30.8));
        m.insert("TR", (39.0, 35.2));
        m.insert("IR", (32.4, 53.7));
        m.insert("IQ", (33.2, 43.7));
        m.insert("SA", (23.9, 45.1));
        m.insert("UA", (48.4, 31.2));
        m.insert("PK", (30.4, 69.3));
        m.insert("BD", (23.7, 90.4));
        m.insert("ID", (-0.8, 113.9));
        m.insert("KR", (36.0, 128.0));
        m.insert("CO", (4.6, -74.3));
        m.insert("AR", (-38.4, -63.6));
        m.insert("CM", (7.4, 12.4));
        m.insert("AZ", (40.1, 47.6));
        m.insert("SB", (-9.6, 160.2));
        m.insert("SY", (35.0, 38.0));
        m.insert("YE", (15.6, 48.5));
        m.insert("LY", (26.3, 17.2));
        m.insert("SD", (12.9, 30.2));
        m.insert("ET", (9.1, 40.5));
        m.insert("MM", (21.9, 95.9));
        m.insert("CU", (21.5, -77.8));
        m.insert("VE", (6.4, -66.6));
        m
    });

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_outage_event() {
        let feed = IodaFeed::new();
        let raw = RawFeedEvent {
            id: EventId::default(),
            timestamp: Utc::now(),
            source: FeedSource::new("ioda-outages"),
            feed_schema: SchemaKey::new("ioda.outages.v1"),
            location: Some(GeoPoint::new(7.4, 12.4)),
            payload: json!({
                "country_code": "CM",
                "country_name": "Cameroon",
                "level": "critical",
                "value": 974,
                "history_value": 985,
            }),
        };

        let events = feed.normalize(&raw).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        match &event.kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert!(summary.contains("Cameroon"));
                assert!(summary.contains("critical"));
            }
            _ => panic!("expected WorldObservation"),
        }
        assert_eq!(event.domain, Some(StateDomain::Infrastructure));
        assert!((event.severity.unwrap() - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn severity_by_level() {
        let feed = IodaFeed::new();
        let make = |level: &str| {
            let raw = RawFeedEvent {
                id: EventId::default(),
                timestamp: Utc::now(),
                source: FeedSource::new("ioda"),
                feed_schema: SchemaKey::new("ioda.outages.v1"),
                location: None,
                payload: json!({"country_code": "US", "country_name": "USA", "level": level}),
            };
            feed.normalize(&raw).unwrap()[0].severity.unwrap()
        };
        assert!((make("critical") - 0.9).abs() < f32::EPSILON);
        assert!((make("warning") - 0.6).abs() < f32::EPSILON);
        assert!((make("normal") - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn country_centroid_lookup() {
        assert!(COUNTRY_CENTROIDS.get("CM").is_some());
        let (lat, lon) = COUNTRY_CENTROIDS["CM"];
        assert!((lat - 7.4).abs() < 0.1);
        assert!((lon - 12.4).abs() < 0.1);
    }

    #[test]
    fn source_and_schema() {
        let feed = IodaFeed::new();
        assert_eq!(feed.source().to_string(), "ioda-outages");
        assert_eq!(feed.schema().to_string(), "ioda.outages.v1");
    }
}
