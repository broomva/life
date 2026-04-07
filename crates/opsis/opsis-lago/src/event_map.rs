//! Bidirectional mapping between OpsisEvent and Lago EventEnvelope.
//!
//! Opsis events are stored as `EventKind::Custom` payloads in Lago, with
//! the full OpsisEvent JSON-serialized under `"opsis.event"` key.

use lago_core::event::{EventEnvelope, EventPayload};
use lago_core::id::{BranchId, EventId as LagoEventId, SessionId};
use opsis_core::event::OpsisEvent;
use std::collections::HashMap;

/// The Lago event kind discriminant used for Opsis events.
const OPSIS_EVENT_KEY: &str = "opsis.event";

/// Convert an OpsisEvent into a Lago EventEnvelope for persistence.
pub fn opsis_to_lago(
    event: &OpsisEvent,
    session_id: &SessionId,
    branch_id: &BranchId,
) -> EventEnvelope {
    let payload = EventPayload::Custom {
        event_type: OPSIS_EVENT_KEY.into(),
        data: serde_json::to_value(event).unwrap_or_default(),
    };

    let mut metadata = HashMap::new();
    metadata.insert("opsis_event_id".into(), event.id.0.clone());
    if let Some(ref domain) = event.domain {
        metadata.insert("opsis_domain".into(), format!("{domain}"));
    }

    EventEnvelope {
        event_id: LagoEventId::new(),
        session_id: session_id.clone(),
        branch_id: branch_id.clone(),
        run_id: None,
        seq: 0, // Assigned by journal on append
        timestamp: EventEnvelope::now_micros(),
        parent_id: None,
        payload,
        metadata,
        schema_version: 1,
    }
}

/// Attempt to extract an OpsisEvent from a Lago EventEnvelope.
///
/// Returns `None` if the envelope doesn't contain an Opsis event.
pub fn lago_to_opsis(envelope: &EventEnvelope) -> Option<OpsisEvent> {
    match &envelope.payload {
        EventPayload::Custom { event_type, data } if event_type == OPSIS_EVENT_KEY => {
            serde_json::from_value(data.clone()).ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use opsis_core::clock::WorldTick;
    use opsis_core::event::{EventSource, OpsisEventKind};
    use opsis_core::feed::{FeedSource, SchemaKey};
    use opsis_core::spatial::GeoPoint;
    use opsis_core::state::StateDomain;

    fn sample_event() -> OpsisEvent {
        OpsisEvent {
            id: opsis_core::event::EventId::default(),
            tick: WorldTick(42),
            timestamp: Utc::now(),
            source: EventSource::Feed(FeedSource::new("usgs-earthquake")),
            kind: OpsisEventKind::WorldObservation {
                summary: "M5.2 earthquake in Alaska".into(),
            },
            location: Some(GeoPoint::new(61.2, -149.9)),
            domain: Some(StateDomain::Emergency),
            severity: Some(0.65),
            schema_key: SchemaKey::new("usgs.geojson.v1"),
            tags: vec!["earthquake".into(), "seismic".into()],
        }
    }

    #[test]
    fn roundtrip_opsis_event() {
        let event = sample_event();
        let session_id = SessionId::from_string("opsis-world");
        let branch_id = BranchId::from("main");

        let envelope = opsis_to_lago(&event, &session_id, &branch_id);
        assert_eq!(envelope.session_id, session_id);
        assert_eq!(envelope.branch_id, branch_id);
        assert_eq!(
            envelope.metadata.get("opsis_event_id").unwrap(),
            &event.id.0
        );
        assert_eq!(envelope.metadata.get("opsis_domain").unwrap(), "Emergency");

        let restored = lago_to_opsis(&envelope).expect("should extract OpsisEvent");
        assert_eq!(restored.id, event.id);
        assert_eq!(restored.domain, Some(StateDomain::Emergency));
        assert!((restored.severity.unwrap() - 0.65).abs() < f32::EPSILON);
    }

    #[test]
    fn non_opsis_envelope_returns_none() {
        let envelope = EventEnvelope {
            event_id: LagoEventId::new(),
            session_id: SessionId::from_string("other"),
            branch_id: BranchId::from("main"),
            run_id: None,
            seq: 1,
            timestamp: EventEnvelope::now_micros(),
            parent_id: None,
            payload: EventPayload::Custom {
                event_type: "arcan.text_delta".into(),
                data: serde_json::json!({"delta": "hello"}),
            },
            metadata: HashMap::new(),
            schema_version: 1,
        };
        assert!(lago_to_opsis(&envelope).is_none());
    }
}
