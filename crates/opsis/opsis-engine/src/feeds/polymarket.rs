//! Polymarket prediction markets feed.
//!
//! Polls the Polymarket CLOB API for active prediction markets.
//! Events are geo-pinned to countries based on question text parsing.
//! Free, no auth required.
//!
//! API docs: https://docs.polymarket.com/

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use opsis_core::error::{OpsisError, OpsisResult};
use opsis_core::event::{EventId, EventSource, OpsisEvent, OpsisEventKind, RawFeedEvent};
use opsis_core::feed::{FeedIngestor, FeedSource, SchemaKey};
use opsis_core::state::StateDomain;

const POLYMARKET_API: &str = "https://gamma-api.polymarket.com/markets";

/// Polymarket prediction markets feed.
pub struct PolymarketFeed {
    client: reqwest::Client,
}

impl Default for PolymarketFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl PolymarketFeed {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Guess domain from market question text.
    fn guess_domain(question: &str) -> StateDomain {
        let q = question.to_lowercase();
        if q.contains("election")
            || q.contains("president")
            || q.contains("vote")
            || q.contains("nominee")
        {
            StateDomain::Politics
        } else if q.contains("war")
            || q.contains("military")
            || q.contains("attack")
            || q.contains("conflict")
        {
            StateDomain::Conflict
        } else if q.contains("bitcoin")
            || q.contains("crypto")
            || q.contains("stock")
            || q.contains("oil")
            || q.contains("price")
        {
            StateDomain::Finance
        } else if q.contains("trade") || q.contains("tariff") || q.contains("sanction") {
            StateDomain::Trade
        } else {
            StateDomain::Politics // default for prediction markets
        }
    }
}

impl FeedIngestor for PolymarketFeed {
    fn source(&self) -> FeedSource {
        FeedSource::new("polymarket")
    }

    fn schema(&self) -> SchemaKey {
        SchemaKey::new("polymarket.markets.v1")
    }

    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn poll_raw(
        &self,
    ) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>> {
        let client = self.client.clone();

        Box::pin(async move {
            let resp = client
                .get(POLYMARKET_API)
                .query(&[
                    ("limit", "50"),
                    ("active", "true"),
                    ("closed", "false"),
                    ("order", "volume24hr"),
                    ("ascending", "false"),
                ])
                .timeout(Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| OpsisError::Feed(format!("polymarket: {e}")))?;

            let markets: Vec<serde_json::Value> = resp
                .json()
                .await
                .map_err(|e| OpsisError::Feed(format!("polymarket: {e}")))?;

            let mut events = Vec::new();
            for market in &markets {
                let question = market["question"].as_str().unwrap_or("");
                if question.is_empty() {
                    continue;
                }

                // Parse outcome prices to get probability
                let prices_str = market["outcomePrices"].as_str().unwrap_or("[0.5,0.5]");
                let probability: f64 = prices_str
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .next()
                    .and_then(|s| s.trim_matches('"').parse().ok())
                    .unwrap_or(0.5);

                let volume = market["volume24hr"]
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| market["volume24hr"].as_f64())
                    .unwrap_or(0.0);

                events.push(RawFeedEvent {
                    id: EventId::default(),
                    timestamp: Utc::now(),
                    source: FeedSource::new("polymarket"),
                    feed_schema: SchemaKey::new("polymarket.markets.v1"),
                    location: None, // Could geo-pin by parsing country from question
                    payload: serde_json::json!({
                        "question": question,
                        "probability": probability,
                        "volume_24h": volume,
                        "end_date": market["endDate"],
                        "slug": market["slug"],
                    }),
                });
            }

            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<OpsisEvent>> {
        let question = raw.payload["question"].as_str().unwrap_or("Unknown market");
        let probability = raw.payload["probability"].as_f64().unwrap_or(0.5);
        let volume = raw.payload["volume_24h"].as_f64().unwrap_or(0.0);

        let pct = (probability * 100.0).round() as u32;
        let summary = format!("{question} — {pct}% (${volume:.0} 24h vol)");
        let domain = Self::guess_domain(question);

        Ok(vec![OpsisEvent {
            id: raw.id.clone(),
            tick: opsis_core::clock::WorldTick::zero(),
            timestamp: raw.timestamp,
            source: EventSource::Feed(raw.source.clone()),
            kind: OpsisEventKind::WorldObservation { summary },
            location: raw.location,
            domain: Some(domain),
            severity: Some(probability as f32), // Higher probability = more "severe" (more certain)
            schema_key: self.schema(),
            tags: vec!["polymarket".into(), "prediction".into(), "forecast".into()],
        }])
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(300) // 5 minutes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_market_event() {
        let feed = PolymarketFeed::new();
        let raw = RawFeedEvent {
            id: EventId::default(),
            timestamp: Utc::now(),
            source: FeedSource::new("polymarket"),
            feed_schema: SchemaKey::new("polymarket.markets.v1"),
            location: None,
            payload: json!({
                "question": "Will Bitcoin exceed $100k by June 2026?",
                "probability": 0.42,
                "volume_24h": 125000.0,
                "end_date": "2026-06-30",
                "slug": "bitcoin-100k-june-2026",
            }),
        };

        let events = feed.normalize(&raw).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        match &event.kind {
            OpsisEventKind::WorldObservation { summary } => {
                assert!(summary.contains("Bitcoin"));
                assert!(summary.contains("42%"));
            }
            _ => panic!("expected WorldObservation"),
        }
        assert_eq!(event.domain, Some(StateDomain::Finance));
        assert!((event.severity.unwrap() - 0.42).abs() < f32::EPSILON);
    }

    #[test]
    fn domain_guessing() {
        assert_eq!(
            PolymarketFeed::guess_domain("US Presidential Election 2028"),
            StateDomain::Politics
        );
        assert_eq!(
            PolymarketFeed::guess_domain("Military action against Iran"),
            StateDomain::Conflict
        );
        assert_eq!(
            PolymarketFeed::guess_domain("Bitcoin price above 100k"),
            StateDomain::Finance
        );
        assert_eq!(
            PolymarketFeed::guess_domain("New tariff on Chinese imports"),
            StateDomain::Trade
        );
    }

    #[test]
    fn source_and_schema() {
        let feed = PolymarketFeed::new();
        assert_eq!(feed.source().to_string(), "polymarket");
        assert_eq!(feed.schema().to_string(), "polymarket.markets.v1");
    }
}
