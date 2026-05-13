//! Lago bridge for Chronos.
//!
//! Records wake events as `EventKind::Custom { event_type: "chronos.wake", data: … }` entries
//! in the lago journal. The `chronos.*` namespace mirrors the pattern used by autonomic
//! (`autonomic.*`) and haima (`finance.*`) — a typed `EventKind::ChronosWake` variant is
//! deliberately deferred until the contract stabilizes after M2-M3.
//!
//! ## Why Custom for now?
//!
//! The kernel-level `EventKind` enum in `aios-protocol` is a stability surface that crosses
//! every substrate. Adding a variant means coordinating a release across all consumers.
//! `Custom { event_type, data }` is the contract-stable escape: chronos can iterate on its
//! payload shape without coordinating an EventKind rev. When the wake event shape stops
//! changing (post-M3), graduate to `EventKind::ChronosWake` via a separate aios-protocol PR.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::HashMap;
use std::sync::Arc;

use aios_protocol::EventKind;
use chronos_core::WakeEvent;
use lago_core::error::LagoError;
use lago_core::event::EventEnvelope;
use lago_core::id::{BranchId, EventId, SeqNo, SessionId};
use lago_core::journal::Journal;
use tracing::{debug, instrument};

/// Lago `event_type` written for chronos wake events. The kernel does not yet have a typed
/// variant; this string is the stable identifier downstream consumers (replay, projections,
/// dashboards) key on.
pub const CHRONOS_WAKE_EVENT_TYPE: &str = "chronos.wake";

/// Default session id used when a wake event has no `target_session` set. Routed to a
/// dedicated "system" session so heartbeat noise doesn't pollute real user sessions.
pub const CHRONOS_SYSTEM_SESSION: &str = "chronos.system";

/// Default branch used when chronosd writes its system-session wakes.
pub const CHRONOS_DEFAULT_BRANCH: &str = "main";

/// Append a [`WakeEvent`] to the lago journal.
///
/// Resolves the target session by precedence:
///
/// 1. `event.target_session` if `Some` (converted from `aios_protocol::SessionId` to
///    `lago_core::id::SessionId` via `from_string`).
/// 2. `default_session` otherwise.
///
/// The branch is always `default_branch` — Chronos M0 doesn't model per-event branching.
#[instrument(skip(journal, event), fields(
    chronos.wake.source = event.source.as_str(),
    chronos.wake.event_id = %event.event_id,
))]
pub async fn record_wake(
    journal: Arc<dyn Journal>,
    event: &WakeEvent,
    default_session: &SessionId,
    default_branch: &BranchId,
) -> Result<SeqNo, LagoError> {
    let session_id = match event.target_session.as_ref() {
        Some(s) => SessionId::from_string(s.as_str()),
        None => default_session.clone(),
    };

    let data = serde_json::json!({
        "event_id": event.event_id.as_str(),
        "fired_at_unix_ms": event.fired_at_unix_ms,
        "source": event.source.as_str(),
        "payload": event.payload,
        "target_session": event.target_session.as_ref().map(|s| s.as_str()),
    });

    let envelope = EventEnvelope {
        event_id: EventId::new(),
        session_id,
        branch_id: default_branch.clone(),
        run_id: None,
        seq: 0, // assigned by the journal
        timestamp: EventEnvelope::now_micros(),
        parent_id: None,
        payload: EventKind::Custom {
            event_type: CHRONOS_WAKE_EVENT_TYPE.to_string(),
            data,
        },
        metadata: HashMap::new(),
        schema_version: 1,
    };

    let seq = journal.append(envelope).await?;
    debug!(seq, "chronos.wake appended");
    Ok(seq)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use aios_protocol::EventKind;
    use chronos_core::{WakeEvent, WakeSource};
    use lago_core::id::{BranchId, SessionId};
    use lago_core::journal::{EventQuery, Journal};
    use lago_journal::RedbJournal;

    use super::*;

    fn open_journal(dir: &Path) -> Arc<dyn Journal> {
        Arc::new(RedbJournal::open(dir.join("test.redb")).expect("open redb")) as Arc<dyn Journal>
    }

    #[tokio::test]
    async fn record_wake_roundtrips_through_lago() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = open_journal(dir.path());
        let session = SessionId::from_string(CHRONOS_SYSTEM_SESSION);
        let branch = BranchId::from_string(CHRONOS_DEFAULT_BRANCH);

        let event = WakeEvent::new(WakeSource::Heartbeat)
            .with_payload(serde_json::json!({ "interval_ms": 5000_u64 }));

        let seq = record_wake(journal.clone(), &event, &session, &branch)
            .await
            .expect("append");
        assert!(seq > 0, "journal should assign a positive sequence");

        let query = EventQuery::new()
            .session(session.clone())
            .branch(branch.clone());
        let stored = journal.read(query).await.expect("read");
        assert_eq!(stored.len(), 1, "exactly one wake should be persisted");

        match &stored[0].payload {
            EventKind::Custom { event_type, data } => {
                assert_eq!(event_type, CHRONOS_WAKE_EVENT_TYPE);
                assert_eq!(data["source"], "heartbeat");
                assert_eq!(data["payload"]["interval_ms"], 5000);
                assert_eq!(data["event_id"], event.event_id.as_str());
            }
            other => panic!("expected EventKind::Custom, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn record_wake_routes_to_target_session_when_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = open_journal(dir.path());
        let fallback = SessionId::from_string(CHRONOS_SYSTEM_SESSION);
        let branch = BranchId::from_string(CHRONOS_DEFAULT_BRANCH);

        let target = aios_protocol::SessionId::from_string("user-session-xyz");
        let event = WakeEvent::new(WakeSource::Http).with_target_session(target);

        let _seq = record_wake(journal.clone(), &event, &fallback, &branch)
            .await
            .expect("append");

        // The wake should live in the user session, not the fallback.
        let user_q = EventQuery::new()
            .session(SessionId::from_string("user-session-xyz"))
            .branch(branch.clone());
        let user_events = journal.read(user_q).await.expect("read user session");
        assert_eq!(user_events.len(), 1, "wake routed to user session");

        let fallback_q = EventQuery::new()
            .session(fallback.clone())
            .branch(branch.clone());
        let fallback_events = journal.read(fallback_q).await.expect("read fallback");
        assert!(
            fallback_events.is_empty(),
            "fallback session should be empty when target_session is set"
        );
    }

    #[tokio::test]
    async fn record_wake_appends_monotonically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = open_journal(dir.path());
        let session = SessionId::from_string(CHRONOS_SYSTEM_SESSION);
        let branch = BranchId::from_string(CHRONOS_DEFAULT_BRANCH);

        let mut seqs = Vec::new();
        for _ in 0..4 {
            let event = WakeEvent::new(WakeSource::Heartbeat);
            let seq = record_wake(journal.clone(), &event, &session, &branch)
                .await
                .expect("append");
            seqs.push(seq);
        }

        for window in seqs.windows(2) {
            assert!(
                window[1] > window[0],
                "sequences must be monotonically increasing: {seqs:?}"
            );
        }
    }
}
