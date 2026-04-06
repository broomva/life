use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::clock::WorldTick;
use crate::feed::{FeedSource, SchemaKey};
use crate::spatial::{GeoHotspot, GeoPoint};
use crate::state::{StateDomain, Trend};

/// Unique event identifier (ULID by default).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(pub String);

impl Default for EventId {
    fn default() -> Self {
        Self(Ulid::new().to_string())
    }
}

/// Who produced this event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventSource {
    Feed(FeedSource),
    Agent(String),
    Gaia,
    System,
    Universe(String),
}

/// What happened — extensible, forward-compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum OpsisEventKind {
    // Feed observations
    WorldObservation {
        summary: String,
    },

    // Gaia insights (future)
    GaiaCorrelation {
        domains: Vec<StateDomain>,
        description: String,
        confidence: f32,
    },
    GaiaAnomaly {
        domain: StateDomain,
        sigma: f32,
        description: String,
    },

    // Agent actions (future)
    AgentObservation {
        insight: String,
        confidence: f32,
    },
    AgentAlert {
        message: String,
    },

    // Forward-compatible
    Custom {
        event_type: String,
        data: serde_json::Value,
    },
}

/// Universal event envelope for all Opsis events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsisEvent {
    pub id: EventId,
    pub tick: WorldTick,
    pub timestamp: DateTime<Utc>,
    pub source: EventSource,
    pub kind: OpsisEventKind,
    pub location: Option<GeoPoint>,
    pub domain: Option<StateDomain>,
    pub severity: Option<f32>,
    pub schema_key: SchemaKey,
    pub tags: Vec<String>,
}

/// A raw event arriving from an external feed, before normalisation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFeedEvent {
    /// Unique event id.
    pub id: EventId,
    /// When the event was produced by the source.
    pub timestamp: DateTime<Utc>,
    /// Which feed produced this event.
    pub source: FeedSource,
    /// Schema describing the payload format.
    pub feed_schema: SchemaKey,
    /// Optional geographic location.
    pub location: Option<GeoPoint>,
    /// Opaque JSON payload from the feed.
    pub payload: serde_json::Value,
}

/// Changes to a single state line within one tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateLineDelta {
    /// Which domain changed.
    pub domain: StateDomain,
    /// New activity level after this tick.
    pub activity: f32,
    /// New trend after this tick.
    pub trend: Trend,
    /// Events ingested this tick (top-K by severity).
    pub new_events: Vec<OpsisEvent>,
    /// Updated hotspot list.
    pub hotspots: Vec<GeoHotspot>,
}

/// Aggregate delta for one world tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldDelta {
    /// The tick this delta corresponds to.
    pub tick: WorldTick,
    /// Wall-clock time.
    pub timestamp: DateTime<Utc>,
    /// Per-domain deltas (only domains that changed).
    pub state_line_deltas: Vec<StateLineDelta>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_unique() {
        let a = EventId::default();
        let b = EventId::default();
        assert_ne!(a, b);
    }

    #[test]
    fn opsis_event_json_roundtrip() {
        let evt = OpsisEvent {
            id: EventId::default(),
            tick: WorldTick(42),
            timestamp: Utc::now(),
            source: EventSource::Feed(FeedSource::new("test-feed")),
            kind: OpsisEventKind::WorldObservation {
                summary: "Market spike".into(),
            },
            location: Some(GeoPoint::new(4.711, -74.072)),
            domain: Some(StateDomain::Finance),
            severity: Some(0.8),
            schema_key: SchemaKey::new("test.v1"),
            tags: vec!["finance".into(), "spike".into()],
        };
        let json = serde_json::to_string(&evt).unwrap();
        let restored: OpsisEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.domain, Some(StateDomain::Finance));
        assert!((restored.severity.unwrap() - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn world_delta_serializes() {
        let delta = WorldDelta {
            tick: WorldTick(1),
            timestamp: Utc::now(),
            state_line_deltas: vec![],
        };
        let json = serde_json::to_string(&delta).unwrap();
        assert!(json.contains("\"tick\""));
    }

    #[test]
    fn event_source_variants_serialize() {
        let sources = vec![
            EventSource::Feed(FeedSource::new("usgs")),
            EventSource::Agent("arcan-1".into()),
            EventSource::Gaia,
            EventSource::System,
            EventSource::Universe("test-sim".into()),
        ];
        for src in sources {
            let json = serde_json::to_string(&src).unwrap();
            let restored: EventSource = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, src);
        }
    }

    #[test]
    fn opsis_event_kind_tagged_serde() {
        let kind = OpsisEventKind::WorldObservation {
            summary: "test".into(),
        };
        let json = serde_json::to_string(&kind).unwrap();
        assert!(json.contains("\"type\":\"WorldObservation\""));
        let _restored: OpsisEventKind = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn custom_event_kind_roundtrip() {
        let kind = OpsisEventKind::Custom {
            event_type: "my.custom.event".into(),
            data: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let restored: OpsisEventKind = serde_json::from_str(&json).unwrap();
        match restored {
            OpsisEventKind::Custom { event_type, data } => {
                assert_eq!(event_type, "my.custom.event");
                assert_eq!(data["key"], "value");
            }
            _ => panic!("expected Custom variant"),
        }
    }

    #[test]
    fn system_event_no_domain_or_severity() {
        let evt = OpsisEvent {
            id: EventId::default(),
            tick: WorldTick(1),
            timestamp: Utc::now(),
            source: EventSource::System,
            kind: OpsisEventKind::AgentAlert {
                message: "startup".into(),
            },
            location: None,
            domain: None,
            severity: None,
            schema_key: SchemaKey::new("system.v1"),
            tags: vec![],
        };
        let json = serde_json::to_string(&evt).unwrap();
        let restored: OpsisEvent = serde_json::from_str(&json).unwrap();
        assert!(restored.domain.is_none());
        assert!(restored.severity.is_none());
    }
}
