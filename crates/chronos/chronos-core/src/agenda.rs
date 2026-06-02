//! The durable per-session **agenda** (M1).
//!
//! Where [`crate::WakeEvent`] answers *"when should an agent wake?"*, the agenda answers
//! *"what should it do when it wakes?"* — a list of [`AgendaItem`]s, each carrying an
//! `intent`, scoped to a session, surviving daemon restarts.
//!
//! ## The [`AgendaStore`] trait
//!
//! Two implementations exist:
//!
//! - [`InMemoryAgendaStore`] (this crate) — a `Mutex<HashMap<…>>`, used by tests and by the
//!   `chronos-api` integration suite (so the API can be exercised without a lago journal).
//! - `chronos_lago::LagoAgendaStore` — persists each mutation as a `Custom("chronos.agenda.*")`
//!   lago event and rebuilds state by folding the journal (a *pure event projection*, matching
//!   the `haima::FinancialState` / `autonomic::HomeostaticState` precedent). This is what makes
//!   the agenda durable across restarts.
//!
//! The mutating methods (`complete` / `defer` / `cancel`) take only an [`AgendaItemId`], never a
//! session. The lago projection therefore writes every agenda event to a single dedicated ledger
//! stream and folds the whole stream on `list`, filtering by the item's `session_id` field —
//! which is why an id alone is enough to address any item.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ChronosError, ChronosResult, SessionId, WakeSource};

/// Unique identifier for an agenda item.
///
/// ULID-based, so the string sorts by creation time — handy for FIFO tie-breaking within a
/// priority band and for inspecting the journal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgendaItemId(pub String);

impl AgendaItemId {
    /// Mint a new ULID-based agenda item id.
    pub fn new() -> Self {
        Self(ulid::Ulid::new().to_string())
    }

    /// View the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AgendaItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AgendaItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Dispatch priority for an agenda item.
///
/// Ordering (the M1 locked decision): `Urgent` > `Normal` > `Deferrable`, FIFO within a band.
/// See [`AgendaItem::dispatch_cmp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Dispatched before everything else.
    Urgent,
    /// The default band.
    #[default]
    Normal,
    /// Dispatched only once `Urgent` and `Normal` are drained.
    Deferrable,
}

impl Priority {
    /// Lowercase string identifier suitable for log fields and event payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::Urgent => "urgent",
            Priority::Normal => "normal",
            Priority::Deferrable => "deferrable",
        }
    }

    /// Sort rank — lower dispatches first.
    pub fn rank(self) -> u8 {
        match self {
            Priority::Urgent => 0,
            Priority::Normal => 1,
            Priority::Deferrable => 2,
        }
    }
}

/// Lifecycle state of an agenda item.
///
/// `Pending` items with a `not_before_unix_ms` in the future are scheduled but not yet ready
/// (see [`AgendaItem::is_ready`]). `Completed` / `Cancelled` are terminal. `Deferred` is the
/// result of an explicit [`AgendaStore::defer`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgendaItemState {
    /// Awaiting dispatch (the M1 default for a freshly-added item).
    Pending,
    /// The kernel ran the item to completion (M2 marks this; M1 exposes the transition).
    Completed,
    /// Explicitly pushed to a later time via [`AgendaStore::defer`].
    Deferred,
    /// Withdrawn before dispatch.
    Cancelled,
}

impl AgendaItemState {
    /// Lowercase string identifier suitable for log fields and event payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            AgendaItemState::Pending => "pending",
            AgendaItemState::Completed => "completed",
            AgendaItemState::Deferred => "deferred",
            AgendaItemState::Cancelled => "cancelled",
        }
    }

    /// Whether the state is terminal (no further transitions expected).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            AgendaItemState::Completed | AgendaItemState::Cancelled
        )
    }
}

/// Input to [`AgendaStore::add`]. The store assigns the `id`, the initial `Pending` state, and
/// the `created_at_unix_ms` timestamp — callers supply only the intent + routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewAgendaItem {
    /// Session the item (and the wake it fires) is targeted at.
    pub session_id: SessionId,
    /// What the agent should do when it wakes (free-form; the kernel interprets it in M2).
    pub intent: String,
    /// Dispatch priority. Defaults to [`Priority::Normal`].
    #[serde(default)]
    pub priority: Priority,
    /// Where the intent came from. `Http` for the M1 `POST /v1/wake` path.
    pub source: WakeSource,
    /// Earliest dispatch time (ms since epoch). `None` means "ready now".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before_unix_ms: Option<i64>,
}

impl NewAgendaItem {
    /// Construct a normal-priority, ready-now item.
    pub fn new(session_id: SessionId, intent: impl Into<String>, source: WakeSource) -> Self {
        Self {
            session_id,
            intent: intent.into(),
            priority: Priority::Normal,
            source,
            not_before_unix_ms: None,
        }
    }

    /// Set the dispatch priority (builder).
    #[must_use]
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Set the earliest-dispatch time in ms since epoch (builder).
    #[must_use]
    pub fn with_not_before(mut self, not_before_unix_ms: i64) -> Self {
        self.not_before_unix_ms = Some(not_before_unix_ms);
        self
    }
}

/// A durable agenda item — the unit of work the kernel will eventually run on wake (M2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgendaItem {
    /// Unique id, minted on add.
    pub id: AgendaItemId,
    /// Session this item belongs to (and the wake routes to).
    pub session_id: SessionId,
    /// What the agent should do when it wakes.
    pub intent: String,
    /// Dispatch priority.
    pub priority: Priority,
    /// Current lifecycle state.
    pub state: AgendaItemState,
    /// Where the intent came from.
    pub source: WakeSource,
    /// When the item was added (ms since epoch).
    pub created_at_unix_ms: i64,
    /// Earliest dispatch time (ms since epoch); `None` means "ready now".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before_unix_ms: Option<i64>,
}

impl AgendaItem {
    /// Build a fresh `Pending` item from a [`NewAgendaItem`], minting an id and stamping the
    /// creation time.
    pub fn pending_from(new: NewAgendaItem) -> Self {
        Self {
            id: AgendaItemId::new(),
            session_id: new.session_id,
            intent: new.intent,
            priority: new.priority,
            state: AgendaItemState::Pending,
            source: new.source,
            created_at_unix_ms: crate::now_unix_ms(),
            not_before_unix_ms: new.not_before_unix_ms,
        }
    }

    /// Whether the item is ready to dispatch at `now_unix_ms`: `Pending` and either no
    /// `not_before` constraint or one already in the past.
    pub fn is_ready(&self, now_unix_ms: i64) -> bool {
        matches!(self.state, AgendaItemState::Pending)
            && self
                .not_before_unix_ms
                .map(|t| t <= now_unix_ms)
                .unwrap_or(true)
    }

    /// Total dispatch ordering: by [`Priority::rank`], then creation time, then id (a stable
    /// FIFO tie-break, since ULIDs are creation-ordered). Use with `slice::sort_by`.
    pub fn dispatch_cmp(&self, other: &AgendaItem) -> Ordering {
        self.priority
            .rank()
            .cmp(&other.priority.rank())
            .then(self.created_at_unix_ms.cmp(&other.created_at_unix_ms))
            .then_with(|| self.id.as_str().cmp(other.id.as_str()))
    }
}

/// Sort a slice of agenda items into dispatch order (see [`AgendaItem::dispatch_cmp`]).
///
/// Shared by every [`AgendaStore::list`] implementation so the dispatch ordering is defined in
/// exactly one place.
pub fn sort_for_dispatch(items: &mut [AgendaItem]) {
    items.sort_by(AgendaItem::dispatch_cmp);
}

/// Durable per-session store of [`AgendaItem`]s.
///
/// Implementations must be `Send + Sync` so they can live behind an `Arc<dyn AgendaStore>` shared
/// across the daemon's HTTP handlers and wake loop. `#[async_trait]` boxes the returned futures
/// for dyn-compatibility (the wake rate Chronos targets — ≤ 100/sec — makes the boxing cost
/// negligible).
#[async_trait]
pub trait AgendaStore: Send + Sync {
    /// Add a new item to the agenda. Returns the freshly-minted id.
    async fn add(&self, item: NewAgendaItem) -> ChronosResult<AgendaItemId>;

    /// Mark an item completed. Errors with [`ChronosError::NotFound`] if the id is unknown.
    async fn complete(&self, id: &AgendaItemId) -> ChronosResult<()>;

    /// Defer an item to a later time (`until_unix_ms`, ms since epoch). Sets state to
    /// [`AgendaItemState::Deferred`] and updates `not_before_unix_ms`. Errors with
    /// [`ChronosError::NotFound`] if the id is unknown.
    async fn defer(&self, id: &AgendaItemId, until_unix_ms: i64) -> ChronosResult<()>;

    /// Cancel an item. Errors with [`ChronosError::NotFound`] if the id is unknown.
    async fn cancel(&self, id: &AgendaItemId) -> ChronosResult<()>;

    /// List the items for a session, in dispatch order. Terminal (completed/cancelled) items are
    /// included so callers can inspect history; filter on [`AgendaItem::state`] if undesired.
    async fn list(&self, session: &SessionId) -> ChronosResult<Vec<AgendaItem>>;
}

/// In-memory [`AgendaStore`] — the reference implementation used by tests and by the
/// `chronos-api` integration suite. Not durable across restarts; for that, use
/// `chronos_lago::LagoAgendaStore`.
#[derive(Default)]
pub struct InMemoryAgendaStore {
    items: Mutex<HashMap<AgendaItemId, AgendaItem>>,
}

impl InMemoryAgendaStore {
    /// Construct an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AgendaStore for InMemoryAgendaStore {
    async fn add(&self, item: NewAgendaItem) -> ChronosResult<AgendaItemId> {
        let stored = AgendaItem::pending_from(item);
        let id = stored.id.clone();
        let mut guard = self.items.lock().expect("agenda mutex poisoned");
        guard.insert(id.clone(), stored);
        Ok(id)
    }

    async fn complete(&self, id: &AgendaItemId) -> ChronosResult<()> {
        let mut guard = self.items.lock().expect("agenda mutex poisoned");
        let item = guard
            .get_mut(id)
            .ok_or_else(|| ChronosError::NotFound(id.to_string()))?;
        item.state = AgendaItemState::Completed;
        Ok(())
    }

    async fn defer(&self, id: &AgendaItemId, until_unix_ms: i64) -> ChronosResult<()> {
        let mut guard = self.items.lock().expect("agenda mutex poisoned");
        let item = guard
            .get_mut(id)
            .ok_or_else(|| ChronosError::NotFound(id.to_string()))?;
        item.state = AgendaItemState::Deferred;
        item.not_before_unix_ms = Some(until_unix_ms);
        Ok(())
    }

    async fn cancel(&self, id: &AgendaItemId) -> ChronosResult<()> {
        let mut guard = self.items.lock().expect("agenda mutex poisoned");
        let item = guard
            .get_mut(id)
            .ok_or_else(|| ChronosError::NotFound(id.to_string()))?;
        item.state = AgendaItemState::Cancelled;
        Ok(())
    }

    async fn list(&self, session: &SessionId) -> ChronosResult<Vec<AgendaItem>> {
        let guard = self.items.lock().expect("agenda mutex poisoned");
        let mut out: Vec<AgendaItem> = guard
            .values()
            .filter(|i| i.session_id.as_str() == session.as_str())
            .cloned()
            .collect();
        sort_for_dispatch(&mut out);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(s: &str) -> SessionId {
        SessionId::from_string(s)
    }

    fn new_item(session: &str, intent: &str) -> NewAgendaItem {
        NewAgendaItem::new(sess(session), intent, WakeSource::Http)
    }

    #[tokio::test]
    async fn add_then_list_returns_pending_item() {
        let store = InMemoryAgendaStore::new();
        let id = store.add(new_item("s1", "rebuild index")).await.unwrap();

        let items = store.list(&sess("s1")).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, id);
        assert_eq!(items[0].state, AgendaItemState::Pending);
        assert_eq!(items[0].intent, "rebuild index");
        assert_eq!(items[0].priority, Priority::Normal);
        assert_eq!(items[0].source, WakeSource::Http);
        assert!(items[0].created_at_unix_ms >= 0);
        assert!(items[0].not_before_unix_ms.is_none());
    }

    #[tokio::test]
    async fn complete_defer_cancel_transition_state() {
        let store = InMemoryAgendaStore::new();
        let a = store.add(new_item("s", "a")).await.unwrap();
        let b = store.add(new_item("s", "b")).await.unwrap();
        let c = store.add(new_item("s", "c")).await.unwrap();

        store.complete(&a).await.unwrap();
        store.defer(&b, 9_999).await.unwrap();
        store.cancel(&c).await.unwrap();

        let items = store.list(&sess("s")).await.unwrap();
        let by_id: HashMap<_, _> = items.into_iter().map(|i| (i.id.clone(), i)).collect();
        assert_eq!(by_id[&a].state, AgendaItemState::Completed);
        assert_eq!(by_id[&b].state, AgendaItemState::Deferred);
        assert_eq!(by_id[&b].not_before_unix_ms, Some(9_999));
        assert_eq!(by_id[&c].state, AgendaItemState::Cancelled);
    }

    #[tokio::test]
    async fn mutations_on_unknown_id_error_not_found() {
        let store = InMemoryAgendaStore::new();
        let ghost = AgendaItemId("01J0000000000000000000GHOST".to_string());
        assert!(matches!(
            store.complete(&ghost).await,
            Err(ChronosError::NotFound(_))
        ));
        assert!(matches!(
            store.defer(&ghost, 1).await,
            Err(ChronosError::NotFound(_))
        ));
        assert!(matches!(
            store.cancel(&ghost).await,
            Err(ChronosError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn list_filters_by_session() {
        let store = InMemoryAgendaStore::new();
        store.add(new_item("alpha", "a1")).await.unwrap();
        store.add(new_item("alpha", "a2")).await.unwrap();
        store.add(new_item("beta", "b1")).await.unwrap();

        assert_eq!(store.list(&sess("alpha")).await.unwrap().len(), 2);
        assert_eq!(store.list(&sess("beta")).await.unwrap().len(), 1);
        assert_eq!(store.list(&sess("gamma")).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_orders_by_priority_then_fifo() {
        let store = InMemoryAgendaStore::new();
        // Add in deliberately scrambled priority order.
        let normal = store.add(new_item("s", "normal-1")).await.unwrap();
        let deferrable = store
            .add(new_item("s", "deferrable").into_priority(Priority::Deferrable))
            .await
            .unwrap();
        let urgent = store
            .add(new_item("s", "urgent").into_priority(Priority::Urgent))
            .await
            .unwrap();

        let items = store.list(&sess("s")).await.unwrap();
        let order: Vec<&AgendaItemId> = items.iter().map(|i| &i.id).collect();
        assert_eq!(order, vec![&urgent, &normal, &deferrable]);
    }

    #[test]
    fn agenda_item_roundtrips_through_json() {
        let item = AgendaItem::pending_from(new_item("sX", "do the thing").with_not_before(123));
        let json = serde_json::to_string(&item).expect("serialize");
        let back: AgendaItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, item.id);
        assert_eq!(back.intent, "do the thing");
        assert_eq!(back.session_id.as_str(), "sX");
        assert_eq!(back.not_before_unix_ms, Some(123));
        assert_eq!(back.state, AgendaItemState::Pending);
    }

    #[test]
    fn is_ready_respects_not_before_and_state() {
        let mut item = AgendaItem::pending_from(new_item("s", "x"));
        assert!(item.is_ready(0), "pending, no not_before => ready");

        item.not_before_unix_ms = Some(1_000);
        assert!(!item.is_ready(999), "not_before in the future => not ready");
        assert!(item.is_ready(1_000), "not_before reached => ready");

        item.state = AgendaItemState::Completed;
        assert!(!item.is_ready(2_000), "terminal state => never ready");
    }

    // Tiny test-only ergonomic helper so the priority test reads cleanly.
    impl NewAgendaItem {
        fn into_priority(self, p: Priority) -> Self {
            self.with_priority(p)
        }
    }
}
