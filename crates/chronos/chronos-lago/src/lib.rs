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
use async_trait::async_trait;
use chronos_core::{
    AgendaItem, AgendaItemId, AgendaItemState, AgendaStore, ChronosError, ChronosResult,
    KernelDispatcher, NewAgendaItem, WakeEvent, WakeRouter, sort_for_dispatch,
    wake_dispatch_params,
};
use lago_core::error::LagoError;
use lago_core::event::EventEnvelope;
use lago_core::id::{BranchId, EventId, SeqNo, SessionId};
use lago_core::journal::{EventQuery, Journal};
use tracing::{debug, instrument, warn};

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

// ---------------------------------------------------------------------------
// M1 — Agenda store (pure event-projection fold)
// ---------------------------------------------------------------------------

/// Lago `event_type` written when an agenda item is created.
pub const CHRONOS_AGENDA_ADDED_EVENT_TYPE: &str = "chronos.agenda.added";
/// Lago `event_type` written when an agenda item is completed.
pub const CHRONOS_AGENDA_COMPLETED_EVENT_TYPE: &str = "chronos.agenda.completed";
/// Lago `event_type` written when an agenda item is deferred.
pub const CHRONOS_AGENDA_DEFERRED_EVENT_TYPE: &str = "chronos.agenda.deferred";
/// Lago `event_type` written when an agenda item is cancelled.
pub const CHRONOS_AGENDA_CANCELLED_EVENT_TYPE: &str = "chronos.agenda.cancelled";
/// Lago `event_type` written when an agenda item's kernel dispatch errored (M2).
pub const CHRONOS_AGENDA_FAILED_EVENT_TYPE: &str = "chronos.agenda.failed";

/// Dedicated lago session holding the chronos agenda ledger.
///
/// Every `chronos.agenda.*` event lands here regardless of the item's *target* session (which is
/// carried as the `session_id` field inside the event payload). Keeping the agenda in a single
/// stream is what lets [`AgendaStore::complete`] / `defer` / `cancel` address an item by id alone:
/// the projection folds the whole ledger and filters by the item's `session_id` field on
/// [`AgendaStore::list`]. Wake events, by contrast, still route to their target session via
/// [`record_wake`] — the agenda ledger and the wake stream are deliberately separate.
pub const CHRONOS_AGENDA_SESSION: &str = "chronos.agenda";

/// Lago-backed [`AgendaStore`].
///
/// Each mutation appends a `Custom("chronos.agenda.*")` event; reads rebuild state by folding the
/// ledger from scratch — a *pure event projection* (the M1 option (b) decision), mirroring the
/// `haima::FinancialState` and `autonomic::HomeostaticState` precedent. No in-memory cache: every
/// [`AgendaStore::list`] re-derives current state from the journal, so the agenda is automatically
/// durable across `chronosd` restarts (open a fresh store over the same journal and the items are
/// still there).
pub struct LagoAgendaStore {
    journal: Arc<dyn Journal>,
    ledger_session: SessionId,
    branch: BranchId,
}

impl LagoAgendaStore {
    /// Construct a store over `journal`, writing the ledger to [`CHRONOS_AGENDA_SESSION`] on
    /// [`CHRONOS_DEFAULT_BRANCH`].
    pub fn new(journal: Arc<dyn Journal>) -> Self {
        Self {
            journal,
            ledger_session: SessionId::from_string(CHRONOS_AGENDA_SESSION),
            branch: BranchId::from_string(CHRONOS_DEFAULT_BRANCH),
        }
    }

    /// Append one agenda event to the ledger.
    async fn append_event(
        &self,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<SeqNo, LagoError> {
        let envelope = EventEnvelope {
            event_id: EventId::new(),
            session_id: self.ledger_session.clone(),
            branch_id: self.branch.clone(),
            run_id: None,
            seq: 0, // assigned by the journal
            timestamp: EventEnvelope::now_micros(),
            parent_id: None,
            payload: EventKind::Custom {
                event_type: event_type.to_string(),
                data,
            },
            metadata: HashMap::new(),
            schema_version: 1,
        };
        self.journal.append(envelope).await
    }

    /// Fold the agenda ledger into the current item set, keyed by id. `read` returns events in
    /// seq order (the compound key is `session+branch+seq`), so a single forward pass is correct.
    async fn project(&self) -> Result<HashMap<AgendaItemId, AgendaItem>, LagoError> {
        let query = EventQuery::new()
            .session(self.ledger_session.clone())
            .branch(self.branch.clone());
        let events = self.journal.read(query).await?;

        let mut items: HashMap<AgendaItemId, AgendaItem> = HashMap::new();
        for envelope in &events {
            let EventKind::Custom { event_type, data } = &envelope.payload else {
                continue;
            };
            match event_type.as_str() {
                CHRONOS_AGENDA_ADDED_EVENT_TYPE => {
                    match serde_json::from_value::<AgendaItem>(data.clone()) {
                        Ok(item) => {
                            items.insert(item.id.clone(), item);
                        }
                        Err(err) => {
                            warn!(error = %err, "skipping malformed chronos.agenda.added payload");
                        }
                    }
                }
                CHRONOS_AGENDA_COMPLETED_EVENT_TYPE => {
                    if let Some(item) = item_for(&mut items, data) {
                        item.state = AgendaItemState::Completed;
                    }
                }
                CHRONOS_AGENDA_DEFERRED_EVENT_TYPE => {
                    if let Some(item) = item_for(&mut items, data) {
                        item.state = AgendaItemState::Deferred;
                        item.not_before_unix_ms =
                            data.get("not_before_unix_ms").and_then(|v| v.as_i64());
                    }
                }
                CHRONOS_AGENDA_CANCELLED_EVENT_TYPE => {
                    if let Some(item) = item_for(&mut items, data) {
                        item.state = AgendaItemState::Cancelled;
                    }
                }
                CHRONOS_AGENDA_FAILED_EVENT_TYPE => {
                    if let Some(item) = item_for(&mut items, data) {
                        item.state = AgendaItemState::Failed;
                    }
                }
                _ => {}
            }
        }
        Ok(items)
    }

    /// Mirror the `InMemoryAgendaStore` contract: a transition on an unknown id is a
    /// [`ChronosError::NotFound`], not a silently-appended ghost event.
    async fn require_exists(&self, id: &AgendaItemId) -> ChronosResult<()> {
        let items = self.project().await.map_err(agenda_err)?;
        if items.contains_key(id) {
            Ok(())
        } else {
            Err(ChronosError::NotFound(id.to_string()))
        }
    }
}

/// Resolve the `id` field of a transition event back to a mutable item in the projection.
fn item_for<'a>(
    items: &'a mut HashMap<AgendaItemId, AgendaItem>,
    data: &serde_json::Value,
) -> Option<&'a mut AgendaItem> {
    let id = data.get("id").and_then(|v| v.as_str())?;
    items.get_mut(&AgendaItemId(id.to_string()))
}

/// Render a lago error as a [`ChronosError::Agenda`], keeping `chronos-core` free of a
/// `lago-core` dependency.
fn agenda_err(err: LagoError) -> ChronosError {
    ChronosError::Agenda(err.to_string())
}

#[async_trait]
impl AgendaStore for LagoAgendaStore {
    async fn add(&self, item: NewAgendaItem) -> ChronosResult<AgendaItemId> {
        let stored = AgendaItem::pending_from(item);
        let id = stored.id.clone();
        let data =
            serde_json::to_value(&stored).map_err(|e| ChronosError::Agenda(e.to_string()))?;
        self.append_event(CHRONOS_AGENDA_ADDED_EVENT_TYPE, data)
            .await
            .map_err(agenda_err)?;
        Ok(id)
    }

    async fn complete(&self, id: &AgendaItemId) -> ChronosResult<()> {
        self.require_exists(id).await?;
        self.append_event(
            CHRONOS_AGENDA_COMPLETED_EVENT_TYPE,
            serde_json::json!({ "id": id.as_str() }),
        )
        .await
        .map_err(agenda_err)?;
        Ok(())
    }

    async fn defer(&self, id: &AgendaItemId, until_unix_ms: i64) -> ChronosResult<()> {
        self.require_exists(id).await?;
        self.append_event(
            CHRONOS_AGENDA_DEFERRED_EVENT_TYPE,
            serde_json::json!({ "id": id.as_str(), "not_before_unix_ms": until_unix_ms }),
        )
        .await
        .map_err(agenda_err)?;
        Ok(())
    }

    async fn cancel(&self, id: &AgendaItemId) -> ChronosResult<()> {
        self.require_exists(id).await?;
        self.append_event(
            CHRONOS_AGENDA_CANCELLED_EVENT_TYPE,
            serde_json::json!({ "id": id.as_str() }),
        )
        .await
        .map_err(agenda_err)?;
        Ok(())
    }

    async fn fail(&self, id: &AgendaItemId) -> ChronosResult<()> {
        self.require_exists(id).await?;
        self.append_event(
            CHRONOS_AGENDA_FAILED_EVENT_TYPE,
            serde_json::json!({ "id": id.as_str() }),
        )
        .await
        .map_err(agenda_err)?;
        Ok(())
    }

    async fn list(&self, session: &chronos_core::SessionId) -> ChronosResult<Vec<AgendaItem>> {
        let items = self.project().await.map_err(agenda_err)?;
        let mut out: Vec<AgendaItem> = items
            .into_values()
            .filter(|i| i.session_id.as_str() == session.as_str())
            .collect();
        sort_for_dispatch(&mut out);
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// M2 — Kernel wake loop (WakeRouter → dispatch → agenda transition)
// ---------------------------------------------------------------------------

/// Drive wakes from `router` into the kernel and record each agenda outcome — the M2 wake loop.
///
/// For every wake the loop:
/// 1. journals it via [`record_wake`] (`chronos.wake` lands in lago, exactly as M0/M1);
/// 2. if it carries an `intent` ([`wake_dispatch_params`]), calls
///    `dispatcher.dispatch(session, intent)` — the kernel runs an actual agent tick;
/// 3. records the outcome on the wake's agenda item: [`AgendaStore::complete`] on success,
///    [`AgendaStore::fail`] on error.
///
/// Bare pulses (heartbeats — no `intent`) are journaled but never dispatched. The loop runs until
/// `router` is exhausted (every trigger returned `None` and the router was shut down from another
/// task). Host it wherever a [`KernelDispatcher`] lives — arcand embeds it behind `--chronos`.
///
/// `default_session` / `default_branch` are the lago routing fallbacks (the `chronos.system`
/// session on `main`) used when a wake has no `target_session`.
pub async fn run_kernel_wake_loop(
    router: &mut WakeRouter,
    journal: Arc<dyn Journal>,
    agenda: &dyn AgendaStore,
    dispatcher: &dyn KernelDispatcher,
    default_session: &SessionId,
    default_branch: &BranchId,
) {
    while let Some(wake) = router.next_wake().await {
        // 1. Journal the wake (chronos.wake → target/system session).
        if let Err(err) = record_wake(journal.clone(), &wake, default_session, default_branch).await
        {
            warn!(error = %err, "failed to record wake; continuing");
        }

        // 2. Only wakes carrying an intent dispatch to the kernel.
        let Some(params) = wake_dispatch_params(&wake) else {
            continue;
        };
        let session = params
            .target_session
            .clone()
            .unwrap_or_else(|| chronos_core::SessionId::from_string(default_session.as_str()));

        let outcome = match dispatcher.dispatch(&session, &params.intent).await {
            Ok(outcome) => outcome,
            Err(err) => chronos_core::DispatchOutcome::failed(err.to_string()),
        };

        // 3. Record the outcome on the agenda item, if the wake carried one.
        if let Some(item_id) = &params.agenda_item_id {
            let transition = if outcome.completed {
                agenda.complete(item_id).await
            } else {
                agenda.fail(item_id).await
            };
            if let Err(err) = transition {
                warn!(
                    error = %err,
                    item = item_id.as_str(),
                    "failed to record agenda transition"
                );
            }
        }

        match &outcome.error {
            Some(err) => warn!(session = session.as_str(), error = %err, "kernel dispatch failed"),
            None => debug!(
                session = session.as_str(),
                tokens = outcome.total_tokens,
                "kernel dispatch completed"
            ),
        }
    }
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

#[cfg(test)]
mod agenda_tests {
    use std::path::Path;
    use std::sync::Arc;

    use aios_protocol::EventKind;
    use chronos_core::{
        AgendaItemState, AgendaStore, NewAgendaItem, Priority, SessionId, WakeSource,
    };
    use lago_core::id::{BranchId, SessionId as LagoSessionId};
    use lago_core::journal::{EventQuery, Journal};
    use lago_journal::RedbJournal;

    use super::*;

    fn open_journal(dir: &Path) -> Arc<dyn Journal> {
        Arc::new(RedbJournal::open(dir.join("agenda.redb")).expect("open redb")) as Arc<dyn Journal>
    }

    fn new_item(session: &str, intent: &str) -> NewAgendaItem {
        NewAgendaItem::new(SessionId::from_string(session), intent, WakeSource::Http)
    }

    #[tokio::test]
    async fn add_then_list_roundtrips_through_lago() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LagoAgendaStore::new(open_journal(dir.path()));

        let id = store
            .add(new_item("user-1", "summarize the inbox").with_priority(Priority::Urgent))
            .await
            .expect("add");

        let items = store
            .list(&SessionId::from_string("user-1"))
            .await
            .expect("list");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, id);
        assert_eq!(items[0].state, AgendaItemState::Pending);
        assert_eq!(items[0].intent, "summarize the inbox");
        assert_eq!(items[0].priority, Priority::Urgent);
        assert_eq!(items[0].source, WakeSource::Http);
    }

    #[tokio::test]
    async fn complete_defer_cancel_fold_into_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LagoAgendaStore::new(open_journal(dir.path()));

        let a = store.add(new_item("s", "a")).await.unwrap();
        let b = store.add(new_item("s", "b")).await.unwrap();
        let c = store.add(new_item("s", "c")).await.unwrap();

        store.complete(&a).await.unwrap();
        store.defer(&b, 8_888).await.unwrap();
        store.cancel(&c).await.unwrap();

        let items = store.list(&SessionId::from_string("s")).await.unwrap();
        let by_id: std::collections::HashMap<_, _> =
            items.into_iter().map(|i| (i.id.clone(), i)).collect();
        assert_eq!(by_id[&a].state, AgendaItemState::Completed);
        assert_eq!(by_id[&b].state, AgendaItemState::Deferred);
        assert_eq!(by_id[&b].not_before_unix_ms, Some(8_888));
        assert_eq!(by_id[&c].state, AgendaItemState::Cancelled);
    }

    #[tokio::test]
    async fn mutations_on_unknown_id_error_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LagoAgendaStore::new(open_journal(dir.path()));
        let ghost = chronos_core::AgendaItemId("01J0000000000000000000GHOST".to_string());
        assert!(matches!(
            store.complete(&ghost).await,
            Err(chronos_core::ChronosError::NotFound(_))
        ));
        assert!(matches!(
            store.defer(&ghost, 1).await,
            Err(chronos_core::ChronosError::NotFound(_))
        ));
        assert!(matches!(
            store.cancel(&ghost).await,
            Err(chronos_core::ChronosError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn list_filters_by_target_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LagoAgendaStore::new(open_journal(dir.path()));
        store.add(new_item("alpha", "a1")).await.unwrap();
        store.add(new_item("alpha", "a2")).await.unwrap();
        store.add(new_item("beta", "b1")).await.unwrap();

        assert_eq!(
            store
                .list(&SessionId::from_string("alpha"))
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store
                .list(&SessionId::from_string("beta"))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// The durability guarantee: a fresh store over the SAME journal sees the agenda — i.e. it
    /// survives a daemon restart. This is the whole point of the lago projection.
    #[tokio::test]
    async fn agenda_survives_store_recreation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = open_journal(dir.path());

        let id = {
            let store = LagoAgendaStore::new(journal.clone());
            let id = store.add(new_item("persist", "remember me")).await.unwrap();
            store.complete(&id).await.unwrap();
            id
        };

        // Re-open a brand new store over the same journal — simulating a chronosd restart.
        let reopened = LagoAgendaStore::new(journal.clone());
        let items = reopened
            .list(&SessionId::from_string("persist"))
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, id);
        assert_eq!(items[0].state, AgendaItemState::Completed);
    }

    /// Agenda events land in the dedicated `chronos.agenda` ledger session (not the item's target
    /// session), so an id alone is enough to address any item across sessions.
    #[tokio::test]
    async fn added_event_is_journaled_under_the_agenda_ledger() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = open_journal(dir.path());
        let store = LagoAgendaStore::new(journal.clone());
        store.add(new_item("user-xyz", "do work")).await.unwrap();

        let ledger = journal
            .read(
                EventQuery::new()
                    .session(LagoSessionId::from_string(CHRONOS_AGENDA_SESSION))
                    .branch(BranchId::from_string(CHRONOS_DEFAULT_BRANCH)),
            )
            .await
            .expect("read ledger");
        assert_eq!(ledger.len(), 1, "one agenda.added event in the ledger");
        match &ledger[0].payload {
            EventKind::Custom { event_type, data } => {
                assert_eq!(event_type, CHRONOS_AGENDA_ADDED_EVENT_TYPE);
                assert_eq!(data["session_id"], "user-xyz");
                assert_eq!(data["intent"], "do work");
                assert_eq!(data["state"], "pending");
            }
            other => panic!("expected EventKind::Custom, got {other:?}"),
        }

        // The target user session holds no agenda events (only wakes would route there).
        let user = journal
            .read(
                EventQuery::new()
                    .session(LagoSessionId::from_string("user-xyz"))
                    .branch(BranchId::from_string(CHRONOS_DEFAULT_BRANCH)),
            )
            .await
            .expect("read user session");
        assert!(
            user.is_empty(),
            "agenda events do not pollute the target session"
        );
    }
}

#[cfg(test)]
mod wake_loop_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use chronos_core::{
        AgendaItemId, AgendaItemState, AgendaStore, MockKernelDispatcher, NewAgendaItem,
        SessionId as ChronosSessionId, WakeEvent, WakeRouter, WakeSource, WakeTrigger,
    };
    use lago_core::id::{BranchId, SessionId as LagoSessionId};
    use lago_core::journal::{EventQuery, Journal};
    use lago_journal::RedbJournal;

    use super::{
        CHRONOS_DEFAULT_BRANCH, CHRONOS_SYSTEM_SESSION, CHRONOS_WAKE_EVENT_TYPE, LagoAgendaStore,
        run_kernel_wake_loop,
    };

    /// Trigger that emits one configured wake, then exhausts.
    struct OneShotTrigger {
        wake: Option<WakeEvent>,
    }
    impl OneShotTrigger {
        fn new(wake: WakeEvent) -> Self {
            Self { wake: Some(wake) }
        }
    }
    #[async_trait]
    impl WakeTrigger for OneShotTrigger {
        async fn next_wake(&mut self) -> Option<WakeEvent> {
            self.wake.take()
        }
        fn name(&self) -> &'static str {
            "oneshot-test"
        }
    }

    fn open_journal(dir: &std::path::Path) -> Arc<dyn Journal> {
        Arc::new(RedbJournal::open(dir.join("m2.redb")).expect("open redb")) as Arc<dyn Journal>
    }

    /// Poll the agenda until `id` reaches a terminal state (or time out → `None`).
    async fn poll_terminal(
        agenda: &LagoAgendaStore,
        session: &ChronosSessionId,
        id: &AgendaItemId,
    ) -> Option<AgendaItemState> {
        for _ in 0..100 {
            let items = agenda.list(session).await.expect("list");
            if let Some(item) = items.iter().find(|i| &i.id == id)
                && item.state.is_terminal()
            {
                return Some(item.state);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        None
    }

    #[tokio::test]
    async fn fail_transition_roundtrips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LagoAgendaStore::new(open_journal(dir.path()));
        let id = store
            .add(NewAgendaItem::new(
                ChronosSessionId::from_string("s"),
                "x",
                WakeSource::Http,
            ))
            .await
            .unwrap();
        store.fail(&id).await.unwrap();
        let items = store
            .list(&ChronosSessionId::from_string("s"))
            .await
            .unwrap();
        assert_eq!(items[0].state, AgendaItemState::Failed);
    }

    #[tokio::test]
    async fn wake_loop_dispatches_and_completes_agenda_item() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = open_journal(dir.path());
        let agenda = Arc::new(LagoAgendaStore::new(journal.clone()));
        let session = ChronosSessionId::from_string("user-1");
        let id = agenda
            .add(NewAgendaItem::new(
                session.clone(),
                "do work",
                WakeSource::Http,
            ))
            .await
            .unwrap();

        let mut router = WakeRouter::new(8);
        let wake = WakeEvent::new(WakeSource::Http)
            .with_payload(serde_json::json!({ "intent": "do work", "agenda_item_id": id.as_str() }))
            .with_target_session(session.clone());
        router.add_trigger(Box::new(OneShotTrigger::new(wake)));

        let dispatcher = Arc::new(MockKernelDispatcher::completed(100));
        let default_session = LagoSessionId::from_string(CHRONOS_SYSTEM_SESSION);
        let branch = BranchId::from_string(CHRONOS_DEFAULT_BRANCH);

        let agenda_task = agenda.clone();
        let journal_task = journal.clone();
        let dispatcher_task = dispatcher.clone();
        let handle = tokio::spawn(async move {
            run_kernel_wake_loop(
                &mut router,
                journal_task,
                agenda_task.as_ref(),
                dispatcher_task.as_ref(),
                &default_session,
                &branch,
            )
            .await;
        });

        let state = poll_terminal(agenda.as_ref(), &session, &id).await;
        handle.abort();
        assert_eq!(state, Some(AgendaItemState::Completed));

        // The mock was dispatched with the wake's session + intent.
        assert_eq!(
            dispatcher.calls(),
            vec![("user-1".to_string(), "do work".to_string())]
        );

        // chronos.wake was journaled to the target session.
        let wakes = journal
            .read(
                EventQuery::new()
                    .session(LagoSessionId::from_string("user-1"))
                    .branch(BranchId::from_string(CHRONOS_DEFAULT_BRANCH)),
            )
            .await
            .expect("read wakes");
        assert!(
            wakes.iter().any(|e| matches!(
                &e.payload,
                aios_protocol::EventKind::Custom { event_type, .. }
                    if event_type == CHRONOS_WAKE_EVENT_TYPE
            )),
            "the wake should be journaled as chronos.wake in the target session"
        );
    }

    #[tokio::test]
    async fn wake_loop_marks_failed_on_dispatch_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = open_journal(dir.path());
        let agenda = Arc::new(LagoAgendaStore::new(journal.clone()));
        let session = ChronosSessionId::from_string("user-2");
        let id = agenda
            .add(NewAgendaItem::new(
                session.clone(),
                "explode",
                WakeSource::Http,
            ))
            .await
            .unwrap();

        let mut router = WakeRouter::new(8);
        router.add_trigger(Box::new(OneShotTrigger::new(
            WakeEvent::new(WakeSource::Http)
                .with_payload(
                    serde_json::json!({ "intent": "explode", "agenda_item_id": id.as_str() }),
                )
                .with_target_session(session.clone()),
        )));

        let dispatcher = Arc::new(MockKernelDispatcher::failing("kernel boom"));
        let default_session = LagoSessionId::from_string(CHRONOS_SYSTEM_SESSION);
        let branch = BranchId::from_string(CHRONOS_DEFAULT_BRANCH);

        let agenda_task = agenda.clone();
        let handle = tokio::spawn(async move {
            run_kernel_wake_loop(
                &mut router,
                journal,
                agenda_task.as_ref(),
                dispatcher.as_ref(),
                &default_session,
                &branch,
            )
            .await;
        });

        let state = poll_terminal(agenda.as_ref(), &session, &id).await;
        handle.abort();
        assert_eq!(state, Some(AgendaItemState::Failed));
    }
}
