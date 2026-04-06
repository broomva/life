//! Open-Meteo global weather feed ingestor.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use opsis_core::clock::WorldTick;
use opsis_core::error::OpsisResult;
use opsis_core::event::{EventId, EventSource, OpsisEvent, OpsisEventKind, RawFeedEvent};
use opsis_core::feed::{FeedIngestor, FeedSource, SchemaKey};
use opsis_core::spatial::GeoPoint;
use opsis_core::state::StateDomain;

/// Cities monitored for weather conditions.
const WEATHER_CITIES: &[(f64, f64, &str)] = &[
    (4.711, -74.072, "Bogota"),
    (40.7128, -74.006, "New York"),
    (51.5074, -0.1278, "London"),
    (35.6762, 139.6503, "Tokyo"),
    (-33.8688, 151.2093, "Sydney"),
];

const OPEN_METEO_URL: &str = "https://api.open-meteo.com/v1/forecast";

/// Open-Meteo weather feed — polls current conditions for key cities.
pub struct OpenMeteoWeatherFeed {
    client: reqwest::Client,
}

impl Default for OpenMeteoWeatherFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenMeteoWeatherFeed {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Map WMO weather code to normalised severity.
    pub fn weather_code_to_severity(code: i64) -> f32 {
        match code {
            0..=3 => 0.05,
            45..=48 => 0.1,
            51..=57 => 0.15,
            61..=65 => 0.25,
            66..=67 => 0.4,
            71..=77 => 0.3,
            80..=82 => 0.35,
            85..=86 => 0.4,
            95 => 0.6,
            96..=99 => 0.75,
            _ => 0.1,
        }
    }

    fn weather_code_to_description(code: i64) -> &'static str {
        match code {
            0 => "Clear sky",
            1..=3 => "Partly cloudy",
            45..=48 => "Fog",
            51..=57 => "Drizzle",
            61..=65 => "Rain",
            66..=67 => "Freezing rain",
            71..=77 => "Snow",
            80..=82 => "Rain showers",
            85..=86 => "Snow showers",
            95 => "Thunderstorm",
            96..=99 => "Thunderstorm with hail",
            _ => "Unknown",
        }
    }
}

impl FeedIngestor for OpenMeteoWeatherFeed {
    fn source(&self) -> FeedSource {
        FeedSource::new("open-meteo")
    }

    fn schema(&self) -> SchemaKey {
        SchemaKey::new("openmeteo.current.v1")
    }

    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn poll_raw(
        &self,
    ) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>> {
        Box::pin(async {
            let mut events = Vec::new();

            for &(lat, lon, city) in WEATHER_CITIES {
                let url = format!(
                    "{OPEN_METEO_URL}?latitude={lat}&longitude={lon}\
                     &current=temperature_2m,wind_speed_10m,weather_code\
                     &timezone=auto"
                );

                let resp = match self.client.get(&url).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(city, "weather fetch failed: {e}");
                        continue;
                    }
                };

                let data: serde_json::Value = match resp.json().await {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(city, "weather parse failed: {e}");
                        continue;
                    }
                };

                events.push(RawFeedEvent {
                    id: EventId::default(),
                    timestamp: Utc::now(),
                    source: self.source(),
                    feed_schema: self.schema(),
                    location: Some(GeoPoint::new(lat, lon)),
                    payload: serde_json::json!({
                        "city": city,
                        "current": data["current"],
                    }),
                });
            }

            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<OpsisEvent>> {
        let city = raw.payload["city"].as_str().unwrap_or("Unknown");
        let current = &raw.payload["current"];
        let weather_code = current["weather_code"].as_i64().unwrap_or(0);
        let temp = current["temperature_2m"].as_f64().unwrap_or(0.0);
        let wind = current["wind_speed_10m"].as_f64().unwrap_or(0.0);

        let severity = Self::weather_code_to_severity(weather_code);
        let description = Self::weather_code_to_description(weather_code);

        Ok(vec![OpsisEvent {
            id: EventId::default(),
            tick: WorldTick::zero(),
            timestamp: Utc::now(),
            source: EventSource::Feed(raw.source.clone()),
            kind: OpsisEventKind::WorldObservation {
                summary: format!("{city}: {description} ({temp:.1}°C, wind {wind:.0} km/h)"),
            },
            location: raw.location,
            domain: Some(StateDomain::Weather),
            severity: Some(severity),
            schema_key: self.schema(),
            tags: vec!["weather".into(), city.to_lowercase()],
        }])
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(300)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_code_severity_clear_is_low() {
        assert!(OpenMeteoWeatherFeed::weather_code_to_severity(0) < 0.1);
    }

    #[test]
    fn weather_code_severity_thunderstorm_is_high() {
        assert!(OpenMeteoWeatherFeed::weather_code_to_severity(96) > 0.5);
    }

    #[test]
    fn normalize_weather_event() {
        let feed = OpenMeteoWeatherFeed::new();
        let raw = RawFeedEvent {
            id: EventId("weather-1".into()),
            timestamp: Utc::now(),
            source: feed.source(),
            feed_schema: feed.schema(),
            location: Some(GeoPoint::new(4.711, -74.072)),
            payload: serde_json::json!({
                "city": "Bogota",
                "current": {
                    "temperature_2m": 18.5,
                    "wind_speed_10m": 12.0,
                    "weather_code": 61
                }
            }),
        };
        let events = feed.normalize(&raw).unwrap();
        assert_eq!(events.len(), 1);
        match &events[0].kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert!(summary.contains("Bogota"));
                assert!(summary.contains("Rain"));
            }
            other => panic!("expected WorldObservation, got {:?}", other),
        }
        assert_eq!(events[0].domain, Some(StateDomain::Weather));
        assert!(matches!(events[0].source, EventSource::Feed(_)));
    }
}
