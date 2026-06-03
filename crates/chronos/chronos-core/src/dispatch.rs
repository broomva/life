//! Kernel dispatch — the M2 handoff surface.
//!
//! When a wake carries an `intent`, Chronos hands it to the kernel to actually run an agent tick.
//! [`KernelDispatcher`] abstracts that call so `chronos-core` stays free of `arcand` /
//! `aios-runtime` types — the arcand implementation wraps `KernelRuntime::tick_on_branch`. The
//! wake → dispatch → agenda-transition loop that ties this together lives in `chronos-lago`
//! (`run_kernel_wake_loop`), where the journal + [`crate::AgendaStore`] are available.
//!
//! Branch: M2 dispatches always target the `main` branch (the M0 chronos convention), so the
//! trait takes only `(session_id, intent)` — the arcand impl supplies the branch.

use async_trait::async_trait;

use crate::{AgendaItemId, ChronosResult, SessionId, WakeEvent};

/// Outcome of one kernel dispatch (a single agent tick driven by a wake).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DispatchOutcome {
    /// Whether the tick reached a terminal/complete state (vs. erroring out).
    pub completed: bool,
    /// Error detail when the dispatch failed. `None` on success.
    pub error: Option<String>,
    /// Total tokens consumed by the tick (best-effort; 0 when unknown, e.g. mock provider).
    pub total_tokens: u64,
}

impl DispatchOutcome {
    /// A successful dispatch consuming `total_tokens`.
    pub fn completed(total_tokens: u64) -> Self {
        Self {
            completed: true,
            error: None,
            total_tokens,
        }
    }

    /// A failed dispatch carrying an error message.
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            completed: false,
            error: Some(error.into()),
            total_tokens: 0,
        }
    }
}

/// Abstracts the kernel-call surface for a wake-driven agent tick.
///
/// The arcand implementation is the *only* place chronos types meet arcand/aios types: it ensures
/// the session exists, builds a `TickInput { objective: intent, .. }`, calls
/// `KernelRuntime::tick_on_branch(session, BranchId::main(), input)`, and folds the `TickOutput`
/// into a [`DispatchOutcome`].
#[async_trait]
pub trait KernelDispatcher: Send + Sync {
    /// Run one agent tick for `session_id` driven by `intent` (the wake's objective).
    async fn dispatch(
        &self,
        session_id: &SessionId,
        intent: &str,
    ) -> ChronosResult<DispatchOutcome>;
}

/// Parameters extracted from a wake for kernel dispatch (see [`wake_dispatch_params`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeDispatch {
    /// The wake's target session (`None` → the loop's default/system session).
    pub target_session: Option<SessionId>,
    /// What the agent should do — the kernel objective.
    pub intent: String,
    /// The agenda item this wake corresponds to, if any. Used to record the outcome transition.
    pub agenda_item_id: Option<AgendaItemId>,
}

/// Extract dispatch parameters from a wake's payload.
///
/// Returns `None` when the wake has no non-empty `intent` — a bare pulse (e.g. a heartbeat) is
/// journaled but never dispatched to the kernel. M1's `POST /v1/wake` payloads carry both
/// `intent` and `agenda_item_id`, so an HTTP-originated wake yields a full [`WakeDispatch`].
pub fn wake_dispatch_params(wake: &WakeEvent) -> Option<WakeDispatch> {
    let intent = wake.payload.get("intent").and_then(|v| v.as_str())?;
    if intent.trim().is_empty() {
        return None;
    }
    let agenda_item_id = wake
        .payload
        .get("agenda_item_id")
        .and_then(|v| v.as_str())
        .map(|s| AgendaItemId(s.to_string()));
    Some(WakeDispatch {
        // Normalize an empty/whitespace target session to None (→ the loop's default/system
        // session) so a hand-built or non-HTTP wake can't dispatch a tick on an empty session id.
        target_session: wake
            .target_session
            .clone()
            .filter(|s| !s.as_str().trim().is_empty()),
        intent: intent.to_string(),
        agenda_item_id,
    })
}

/// Test [`KernelDispatcher`] — returns a configured outcome and records every call.
#[cfg(any(test, feature = "test-util"))]
pub struct MockKernelDispatcher {
    outcome: DispatchOutcome,
    /// Recorded `(session_id, intent)` calls, in order.
    calls: std::sync::Mutex<Vec<(String, String)>>,
}

#[cfg(any(test, feature = "test-util"))]
impl MockKernelDispatcher {
    /// A mock that always reports completion with `total_tokens`.
    pub fn completed(total_tokens: u64) -> Self {
        Self {
            outcome: DispatchOutcome::completed(total_tokens),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// A mock that always reports failure with `error`.
    pub fn failing(error: impl Into<String>) -> Self {
        Self {
            outcome: DispatchOutcome::failed(error),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The `(session_id, intent)` calls recorded so far.
    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().expect("mock mutex poisoned").clone()
    }
}

#[cfg(any(test, feature = "test-util"))]
#[async_trait]
impl KernelDispatcher for MockKernelDispatcher {
    async fn dispatch(
        &self,
        session_id: &SessionId,
        intent: &str,
    ) -> ChronosResult<DispatchOutcome> {
        self.calls
            .lock()
            .expect("mock mutex poisoned")
            .push((session_id.as_str().to_string(), intent.to_string()));
        Ok(self.outcome.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WakeSource;

    fn http_wake(payload: serde_json::Value) -> WakeEvent {
        WakeEvent::new(WakeSource::Http).with_payload(payload)
    }

    #[test]
    fn dispatch_params_extracts_intent_and_item_id() {
        let wake = http_wake(serde_json::json!({
            "intent": "summarize the inbox",
            "agenda_item_id": "01JITEM",
        }))
        .with_target_session(SessionId::from_string("user-7"));

        let params = wake_dispatch_params(&wake).expect("has intent");
        assert_eq!(params.intent, "summarize the inbox");
        assert_eq!(
            params.agenda_item_id,
            Some(AgendaItemId("01JITEM".to_string()))
        );
        assert_eq!(
            params.target_session.as_ref().map(|s| s.as_str()),
            Some("user-7")
        );
    }

    #[test]
    fn bare_wake_without_intent_does_not_dispatch() {
        // A heartbeat-style payload (no intent) → None → never dispatched.
        let wake = http_wake(serde_json::json!({ "interval_ms": 5000 }));
        assert!(wake_dispatch_params(&wake).is_none());

        // An explicitly-empty intent is also skipped.
        let blank = http_wake(serde_json::json!({ "intent": "   " }));
        assert!(wake_dispatch_params(&blank).is_none());
    }

    #[test]
    fn dispatch_params_without_item_id_is_some_with_none() {
        let wake = http_wake(serde_json::json!({ "intent": "do it" }));
        let params = wake_dispatch_params(&wake).expect("has intent");
        assert!(params.agenda_item_id.is_none());
    }

    #[test]
    fn outcome_constructors() {
        let ok = DispatchOutcome::completed(1234);
        assert!(ok.completed && ok.error.is_none() && ok.total_tokens == 1234);
        let err = DispatchOutcome::failed("boom");
        assert!(!err.completed && err.error.as_deref() == Some("boom"));
    }

    #[tokio::test]
    async fn mock_records_calls_and_returns_outcome() {
        let mock = MockKernelDispatcher::completed(42);
        let out = mock
            .dispatch(&SessionId::from_string("s"), "go")
            .await
            .unwrap();
        assert_eq!(out, DispatchOutcome::completed(42));
        assert_eq!(mock.calls(), vec![("s".to_string(), "go".to_string())]);
    }
}
