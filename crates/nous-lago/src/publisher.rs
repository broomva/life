//! Publishes Nous evaluation events to the Lago journal.

use nous_core::events::NousEvent;
use nous_core::score::EvalScore;
use tracing::debug;

/// Publishes eval events to the Lago journal.
///
/// Thin adapter that converts `EvalScore` / `NousEvent` to
/// `EventKind::Custom` with `"eval."` prefix and appends to the journal.
pub struct NousPublisher;

impl NousPublisher {
    /// Convert an `EvalScore` into a `NousEvent` and then into an `EventKind`.
    pub fn score_to_event_kind(score: &EvalScore) -> aios_protocol::event::EventKind {
        let event = NousEvent::from_inline_score(score);
        event.into_event_kind()
    }

    /// Convert a `NousEvent` directly into an `EventKind`.
    pub fn event_to_event_kind(event: NousEvent) -> aios_protocol::event::EventKind {
        debug!(
            event_type = std::any::type_name::<NousEvent>(),
            "publishing nous event to lago"
        );
        event.into_event_kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::event::EventKind;
    use nous_core::{EvalLayer, EvalTiming};

    #[test]
    fn score_to_event_kind_produces_custom() {
        let score =
            EvalScore::new("test", 0.85, EvalLayer::Execution, EvalTiming::Inline, "s").unwrap();
        let kind = NousPublisher::score_to_event_kind(&score);
        assert!(
            matches!(kind, EventKind::Custom { event_type, .. } if event_type.starts_with("eval."))
        );
    }

    #[test]
    fn event_to_event_kind_produces_custom() {
        let event = NousEvent::QualityChanged {
            session_id: "sess-1".into(),
            aggregate_quality: 0.82,
            trend: 0.01,
            inline_count: 10,
            async_count: 2,
        };
        let kind = NousPublisher::event_to_event_kind(event);
        assert!(
            matches!(kind, EventKind::Custom { event_type, .. } if event_type == "eval.QualityChanged")
        );
    }
}
