//! Subscribes to Nous evaluation events from the Lago journal.

use aios_protocol::event::EventKind;
use nous_core::events::NousEvent;

/// Processes eval events from the Lago journal stream.
///
/// Filters for `"eval."` prefixed custom events and deserializes
/// them back into `NousEvent` variants for downstream processing.
pub struct NousSubscriber;

impl NousSubscriber {
    /// Try to extract a `NousEvent` from an `EventKind`.
    ///
    /// Returns `None` if the event is not a Nous evaluation event.
    pub fn try_extract(kind: &EventKind) -> Option<NousEvent> {
        if let EventKind::Custom { event_type, data } = kind {
            NousEvent::from_custom(event_type, data)
        } else {
            None
        }
    }

    /// Check if an `EventKind` is a Nous evaluation event.
    pub fn is_eval_event(kind: &EventKind) -> bool {
        matches!(kind, EventKind::Custom { event_type, .. } if NousEvent::is_eval_event(event_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::score::ScoreLabel;
    use nous_core::taxonomy::EvalLayer;

    #[test]
    fn extract_inline_completed() {
        let event = NousEvent::InlineCompleted {
            evaluator: "test".into(),
            score: 0.9,
            label: ScoreLabel::Good,
            layer: EvalLayer::Execution,
            session_id: "s".into(),
            run_id: None,
            explanation: None,
        };
        let kind = event.into_event_kind();

        let extracted = NousSubscriber::try_extract(&kind).unwrap();
        assert!(
            matches!(extracted, NousEvent::InlineCompleted { evaluator, .. } if evaluator == "test")
        );
    }

    #[test]
    fn non_eval_event_returns_none() {
        let kind = EventKind::RunFinished {
            reason: "done".into(),
            total_iterations: 1,
            final_answer: None,
            usage: None,
        };
        assert!(NousSubscriber::try_extract(&kind).is_none());
    }

    #[test]
    fn is_eval_event_checks_prefix() {
        let eval_kind = EventKind::Custom {
            event_type: "eval.InlineCompleted".into(),
            data: serde_json::json!({}),
        };
        assert!(NousSubscriber::is_eval_event(&eval_kind));

        let other_kind = EventKind::Custom {
            event_type: "autonomic.CostCharged".into(),
            data: serde_json::json!({}),
        };
        assert!(!NousSubscriber::is_eval_event(&other_kind));
    }
}
