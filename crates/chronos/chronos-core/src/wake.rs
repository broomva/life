//! [`WakeEvent`] and its supporting types.

use aios_protocol::SessionId;
use serde::{Deserialize, Serialize};

/// Unique identifier for a wake event.
///
/// ULIDs are sortable by creation time, which is convenient for ordering wake events when
/// inspecting the journal. Stored as a string in [`WakeEvent`] serialization to keep the
/// representation stable across language boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WakeEventId(pub String);

impl WakeEventId {
    /// Mint a new ULID-based wake event id.
    pub fn new() -> Self {
        Self(ulid::Ulid::new().to_string())
    }

    /// View the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for WakeEventId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WakeEventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Taxonomy of wake sources Chronos coalesces into a single stream.
///
/// Heartbeat is the only source implemented in M0. The others are present so that the
/// universal `WakeEvent` shape doesn't need to change as triggers come online in M1+.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeSource {
    /// Periodic timer-driven wake — chronosd's own pulse.
    Heartbeat,
    /// External HTTP `POST /v1/wake` (M1).
    Http,
    /// Cron expression matched the current minute (M3+).
    Cron,
    /// Filesystem event (create / modify / delete) (M3).
    FsWatch,
    /// A spawned sub-agent returned to its parent's agenda (M3).
    SubAgentReturn,
    /// A metric threshold crossing fired (e.g. Nous score < 0.3) (M3+).
    Threshold,
    /// Inbound webhook from a trusted external system (M3+).
    Webhook,
}

impl WakeSource {
    /// Lowercase string identifier suitable for log fields and event-type suffixes.
    pub fn as_str(self) -> &'static str {
        match self {
            WakeSource::Heartbeat => "heartbeat",
            WakeSource::Http => "http",
            WakeSource::Cron => "cron",
            WakeSource::FsWatch => "fs_watch",
            WakeSource::SubAgentReturn => "sub_agent_return",
            WakeSource::Threshold => "threshold",
            WakeSource::Webhook => "webhook",
        }
    }
}

/// A single wake fired by any of the registered triggers.
///
/// The shape is intentionally open: source-specific data lives in [`WakeEvent::payload`]
/// (a `serde_json::Value`). This avoids growing the type once per trigger source and lets
/// the lago projection layer carry the payload through unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeEvent {
    /// ULID minted at the moment the trigger fired.
    pub event_id: WakeEventId,
    /// Milliseconds since the Unix epoch when the trigger fired.
    pub fired_at_unix_ms: i64,
    /// Which trigger source emitted the wake.
    pub source: WakeSource,
    /// Source-specific data (e.g. heartbeat interval, HTTP body, fs path).
    pub payload: serde_json::Value,
    /// Optional target session — if `None`, the wake is routed to the `chronos.system` session.
    pub target_session: Option<SessionId>,
}

impl WakeEvent {
    /// Construct a wake event with the supplied source and an empty JSON object payload.
    ///
    /// Convenience constructor for the heartbeat / stub cases. Callers needing richer
    /// payloads can construct [`WakeEvent`] directly.
    pub fn new(source: WakeSource) -> Self {
        Self {
            event_id: WakeEventId::new(),
            fired_at_unix_ms: super::now_unix_ms(),
            source,
            payload: serde_json::Value::Object(serde_json::Map::new()),
            target_session: None,
        }
    }

    /// Attach a payload value to a wake event using the builder pattern.
    #[must_use]
    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }

    /// Attach a target session id using the builder pattern.
    #[must_use]
    pub fn with_target_session(mut self, session: SessionId) -> Self {
        self.target_session = Some(session);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_event_id_unique() {
        let a = WakeEventId::new();
        let b = WakeEventId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn wake_source_string_taxonomy_stable() {
        // These string values land in lago payloads. Changing them is a contract break;
        // this test prevents accidental rename.
        assert_eq!(WakeSource::Heartbeat.as_str(), "heartbeat");
        assert_eq!(WakeSource::Http.as_str(), "http");
        assert_eq!(WakeSource::Cron.as_str(), "cron");
        assert_eq!(WakeSource::FsWatch.as_str(), "fs_watch");
        assert_eq!(WakeSource::SubAgentReturn.as_str(), "sub_agent_return");
        assert_eq!(WakeSource::Threshold.as_str(), "threshold");
        assert_eq!(WakeSource::Webhook.as_str(), "webhook");
    }

    #[test]
    fn wake_event_constructor_sets_source_and_empty_payload() {
        let e = WakeEvent::new(WakeSource::Heartbeat);
        assert_eq!(e.source, WakeSource::Heartbeat);
        assert!(e.payload.is_object());
        assert_eq!(e.payload.as_object().map(|o| o.len()), Some(0));
        assert!(e.target_session.is_none());
        assert!(e.fired_at_unix_ms >= 0);
    }

    #[test]
    fn wake_event_builder_attaches_payload_and_target() {
        let e = WakeEvent::new(WakeSource::Http)
            .with_payload(serde_json::json!({"intent": "rebuild_index"}))
            .with_target_session(SessionId::from_string("sess-42"));
        assert_eq!(e.source, WakeSource::Http);
        assert_eq!(e.payload["intent"], "rebuild_index");
        assert_eq!(
            e.target_session.as_ref().map(|s| s.as_str()),
            Some("sess-42")
        );
    }

    #[test]
    fn wake_event_roundtrips_through_json() {
        let e = WakeEvent::new(WakeSource::Heartbeat)
            .with_payload(serde_json::json!({"interval_ms": 5_000_u64}));
        let json = serde_json::to_string(&e).expect("serialize");
        let back: WakeEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.source, WakeSource::Heartbeat);
        assert_eq!(back.event_id, e.event_id);
        assert_eq!(back.payload["interval_ms"], 5_000);
    }
}
