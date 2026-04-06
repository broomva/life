use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::clock::WorldTick;
use crate::feed::FeedSource;
use crate::feed::SchemaKey;
use crate::spatial::GeoHotspot;
use crate::spatial::GeoPoint;
use crate::state::{StateDomain, Trend};

/// Unique event identifier (ULID by default).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(pub String);

impl Default for EventId {
    fn default() -> Self {
        Self(Ulid::new().to_string())
    }
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

/// A normalised state event, ready to influence state lines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateEvent {
    /// Unique id for this state event.
    pub id: EventId,
    /// World tick at which this event was processed.
    pub tick: WorldTick,
    /// The domain this event affects.
    pub domain: StateDomain,
    /// Optional location.
    pub location: Option<GeoPoint>,
    /// Severity / importance (0.0–1.0).
    pub severity: f32,
    /// Human-readable summary.
    pub summary: String,
    /// The feed that produced the raw data.
    pub source: FeedSource,
    /// Freeform tags for filtering.
    pub tags: Vec<String>,
    /// Reference back to the raw event.
    pub raw_ref: EventId,
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
    /// State events that were ingested this tick.
    pub new_events: Vec<StateEvent>,
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
    fn state_event_json_roundtrip() {
        let evt = StateEvent {
            id: EventId::default(),
            tick: WorldTick(42),
            domain: StateDomain::Finance,
            location: Some(GeoPoint::new(4.711, -74.072)),
            severity: 0.8,
            summary: "Market spike".into(),
            source: FeedSource::new("test-feed"),
            tags: vec!["finance".into(), "spike".into()],
            raw_ref: EventId::default(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let restored: StateEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.domain, StateDomain::Finance);
        assert!((restored.severity - 0.8).abs() < f32::EPSILON);
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
}
