//! Stamps [`aios_protocol::event::EventRecord`] headers (session, agent,
//! branch, timestamp) and persists them through the engine's
//! [`aios_protocol::ports::EventStorePort`] trait object.
//!
//! The emitter is the single choke-point through which the kernel's
//! building blocks ([`crate::backend_registry::BackendRegistry`],
//! [`crate::metering::MeteringWrapper`], and — in BRO-870 — the
//! `GateChain` + `KernelEngine`) record state transitions. Keeping a
//! dedicated helper means those call sites do not need to worry about
//! envelope construction or causation wiring; they describe *what*
//! happened, the emitter decides *how* it is recorded.
//!
//! ## Design invariants
//!
//! 1. **Immutable after `build()`.** Every field on [`EventEmitter`]
//!    is set at construction and never changes afterwards. No caller
//!    can mutate session, agent, or branch attribution mid-flight;
//!    this is how the kernel maintains the "zero hidden state"
//!    guarantee at the observation boundary.
//! 2. **Clock is an explicit dependency.** The builder takes an
//!    `Arc<dyn Fn() -> DateTime<Utc>>` so tests can freeze time. A
//!    frozen clock is a necessary precondition for the deterministic
//!    replay test landing in BRO-876.
//! 3. **Events are appended in one shot.** `emit` performs exactly one
//!    `store.append` call per invocation; callers can assume
//!    single-event semantics.

use std::sync::Arc;

use aios_protocol::event::{EventKind, EventRecord};
use aios_protocol::ids::{AgentId, BranchId, EventId, SessionId};
use aios_protocol::kernel::{KernelError, KernelResult};
use aios_protocol::ports::EventStorePort;
use chrono::{DateTime, Utc};

/// Callable clock used by the emitter to stamp
/// [`EventRecord::timestamp`].
///
/// Kept as a type alias so the `EventEmitter` struct signature stays
/// readable and the builder can accept any `Fn() -> DateTime<Utc>`
/// wrapped in `Arc`.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Helper that emits kernel-tier events into an
/// [`aios_protocol::ports::EventStorePort`].
///
/// Construct via [`EventEmitter::builder`]; see the module docs for the
/// surrounding design notes.
#[derive(Clone)]
pub struct EventEmitter {
    store: Arc<dyn EventStorePort>,
    session_id: SessionId,
    branch_id: BranchId,
    agent_id: AgentId,
    clock: Clock,
}

impl EventEmitter {
    /// Start building an [`EventEmitter`] bound to `store`.
    pub fn builder(store: Arc<dyn EventStorePort>) -> EventEmitterBuilder {
        EventEmitterBuilder {
            store,
            session_id: None,
            branch_id: None,
            agent_id: None,
            clock: None,
        }
    }

    /// Append a single event to the store.
    ///
    /// The emitter constructs an [`EventRecord`] stamped with its
    /// session / agent / branch attribution and the clock's current
    /// time. If `causation` is `Some`, it is threaded into
    /// [`EventRecord::causation_id`] so downstream replay tooling can
    /// reconstruct per-dispatch event chains.
    ///
    /// The returned record is the canonical form produced by the event
    /// store (which typically assigns the monotonic sequence number).
    /// Callers that need to chain events should take `event_id` from
    /// that returned record and pass it back in as the next call's
    /// `causation` argument.
    pub async fn emit(
        &self,
        kind: EventKind,
        causation: Option<EventId>,
    ) -> KernelResult<EventRecord> {
        let timestamp = (self.clock)();
        let mut record = EventRecord::new(
            self.session_id.clone(),
            self.branch_id.clone(),
            // Sequence 0 — the store assigns the authoritative seq on
            // append and returns the updated record.
            0,
            kind,
        );
        record.timestamp = timestamp;
        record.agent_id = self.agent_id.clone();
        record.causation_id = causation;

        self.store
            .append(record)
            .await
            // Bridge the legacy `error::KernelError` returned by
            // `EventStorePort::append` into the richer kernel-tier
            // `kernel::KernelError` used throughout this crate.
            .map_err(|e| KernelError::Internal(e.to_string()))
    }

    /// Session this emitter is bound to.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Branch this emitter is bound to.
    pub fn branch_id(&self) -> &BranchId {
        &self.branch_id
    }

    /// Agent this emitter is bound to.
    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
}

/// Staging struct returned by [`EventEmitter::builder`].
///
/// Required fields: [`session`](Self::session),
/// [`agent`](Self::agent). [`branch`](Self::branch) defaults to
/// [`BranchId::main`] and [`clock`](Self::clock) defaults to
/// [`chrono::Utc::now`]. Call [`build`](Self::build) once all required
/// fields are set.
pub struct EventEmitterBuilder {
    store: Arc<dyn EventStorePort>,
    session_id: Option<SessionId>,
    branch_id: Option<BranchId>,
    agent_id: Option<AgentId>,
    clock: Option<Clock>,
}

/// Error returned by [`EventEmitterBuilder::build`] when a required
/// field is missing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// `session(...)` was not called on the builder.
    #[error("EventEmitterBuilder: `session` is required")]
    MissingSession,
    /// `agent(...)` was not called on the builder.
    #[error("EventEmitterBuilder: `agent` is required")]
    MissingAgent,
}

impl EventEmitterBuilder {
    /// Bind the emitter to `session_id`. Required.
    pub fn session(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Bind the emitter to `agent_id`. Required.
    pub fn agent(mut self, agent_id: AgentId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    /// Override the branch (defaults to [`BranchId::main`]).
    pub fn branch(mut self, branch_id: BranchId) -> Self {
        self.branch_id = Some(branch_id);
        self
    }

    /// Override the clock (defaults to [`chrono::Utc::now`]).
    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Finalise the builder.
    ///
    /// Returns [`BuildError`] if a required field was not set.
    pub fn build(self) -> Result<EventEmitter, BuildError> {
        Ok(EventEmitter {
            store: self.store,
            session_id: self.session_id.ok_or(BuildError::MissingSession)?,
            branch_id: self.branch_id.unwrap_or_else(BranchId::main),
            agent_id: self.agent_id.ok_or(BuildError::MissingAgent)?,
            clock: self.clock.unwrap_or_else(|| Arc::new(Utc::now) as Clock),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use aios_protocol::error::KernelResult as LegacyKernelResult;
    use aios_protocol::event::{KernelVmCreated, KernelVmDestroyed};
    use aios_protocol::hypervisor::{BackendId, VmId};
    use aios_protocol::ids::{BranchId, SeqNo};
    use aios_protocol::ports::EventRecordStream;
    use async_trait::async_trait;
    use chrono::TimeZone;

    /// In-memory [`EventStorePort`] used only by the emitter tests.
    ///
    /// Every `append` is copied into an internal `Vec`; the
    /// accompanying [`StubEventStore::stored_events`] helper returns a
    /// snapshot for assertion.
    struct StubEventStore {
        events: Mutex<Vec<EventRecord>>,
    }

    impl StubEventStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Vec::new()),
            })
        }

        fn stored_events(&self) -> Vec<EventRecord> {
            self.events.lock().expect("poisoned mutex").clone()
        }
    }

    #[async_trait]
    impl EventStorePort for StubEventStore {
        async fn append(&self, mut event: EventRecord) -> LegacyKernelResult<EventRecord> {
            let mut buf = self.events.lock().expect("poisoned mutex");
            // Emulate a store-side sequence assignment so tests can
            // verify the returned record has the post-append seq.
            event.sequence = buf.len() as SeqNo;
            buf.push(event.clone());
            Ok(event)
        }

        async fn read(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
            _from_sequence: u64,
            _limit: usize,
        ) -> LegacyKernelResult<Vec<EventRecord>> {
            Ok(self.stored_events())
        }

        async fn head(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
        ) -> LegacyKernelResult<u64> {
            Ok(self.stored_events().len() as u64)
        }

        async fn subscribe(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
            _after_sequence: u64,
        ) -> LegacyKernelResult<EventRecordStream> {
            unimplemented!("subscribe not used in emitter tests")
        }
    }

    fn frozen_clock() -> Clock {
        let fixed = Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0).unwrap();
        Arc::new(move || fixed)
    }

    fn sample_kind() -> EventKind {
        EventKind::KernelVmCreated(KernelVmCreated {
            vm_id: VmId::from("vm-1"),
            backend: BackendId::from("local"),
            spec_hash: "deadbeef".into(),
            session_id: SessionId::from_string("sess-1"),
            agent_id: AgentId::from_string("agent-1"),
        })
    }

    fn build_emitter(store: Arc<StubEventStore>) -> EventEmitter {
        match EventEmitter::builder(store)
            .session(SessionId::from_string("sess-1"))
            .agent(AgentId::from_string("agent-1"))
            .clock(frozen_clock())
            .build()
        {
            Ok(emitter) => emitter,
            Err(e) => panic!("builder failed: {e:?}"),
        }
    }

    #[tokio::test]
    async fn emit_appends_to_store() {
        let store = StubEventStore::new();
        let emitter = build_emitter(store.clone());

        let record = emitter
            .emit(sample_kind(), None)
            .await
            .expect("emit should succeed");

        let stored = store.stored_events();
        assert_eq!(stored.len(), 1);
        // Store stamped the sequence on append; the returned record
        // must carry that same sequence back to the caller.
        assert_eq!(record.sequence, stored[0].sequence);
        // Frozen clock — timestamp must match what the emitter was
        // configured with.
        assert_eq!(record.timestamp, stored[0].timestamp);
        assert_eq!(
            record.timestamp,
            Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0).unwrap()
        );
        assert!(matches!(stored[0].kind, EventKind::KernelVmCreated(_)));
    }

    #[tokio::test]
    async fn emit_stamps_session_agent_ids() {
        let store = StubEventStore::new();
        let emitter = build_emitter(store.clone());

        emitter
            .emit(sample_kind(), None)
            .await
            .expect("emit should succeed");

        // Two more emissions with different kinds, to make sure every
        // record picks up the emitter's attribution.
        emitter
            .emit(
                EventKind::KernelVmDestroyed(KernelVmDestroyed {
                    vm_id: VmId::from("vm-1"),
                    reason: "test".into(),
                }),
                None,
            )
            .await
            .expect("second emit should succeed");

        let stored = store.stored_events();
        assert_eq!(stored.len(), 2);
        for record in &stored {
            assert_eq!(record.session_id, SessionId::from_string("sess-1"));
            assert_eq!(record.agent_id, AgentId::from_string("agent-1"));
            // Default branch is `main`.
            assert_eq!(record.branch_id, BranchId::main());
        }
    }

    #[tokio::test]
    async fn emit_sets_causation_when_provided() {
        let store = StubEventStore::new();
        let emitter = build_emitter(store.clone());

        // First event has no causation.
        let parent = emitter
            .emit(sample_kind(), None)
            .await
            .expect("first emit should succeed");
        assert!(parent.causation_id.is_none());

        // Second event chains via `causation = Some(parent.event_id)`.
        let child = emitter
            .emit(
                EventKind::KernelVmDestroyed(KernelVmDestroyed {
                    vm_id: VmId::from("vm-1"),
                    reason: "test".into(),
                }),
                Some(parent.event_id.clone()),
            )
            .await
            .expect("second emit should succeed");

        assert_eq!(child.causation_id.as_ref(), Some(&parent.event_id));

        let stored = store.stored_events();
        assert_eq!(stored[0].causation_id, None);
        assert_eq!(stored[1].causation_id, Some(parent.event_id));
    }

    #[tokio::test]
    async fn builder_defaults_branch_and_clock() {
        let store = StubEventStore::new();
        let emitter = match EventEmitter::builder(store)
            .session(SessionId::from_string("sess"))
            .agent(AgentId::from_string("agent"))
            .build()
        {
            Ok(e) => e,
            Err(e) => panic!("defaults should satisfy required fields, got {e:?}"),
        };
        assert_eq!(emitter.branch_id(), &BranchId::main());
    }

    #[test]
    fn builder_requires_session_and_agent() {
        let store = StubEventStore::new();
        // Missing session — `EventEmitter` does not implement `Debug`
        // (it holds `Arc<dyn …>` fields), so match manually rather than
        // reaching for `expect_err`.
        match EventEmitter::builder(store.clone())
            .agent(AgentId::from_string("agent"))
            .build()
        {
            Err(BuildError::MissingSession) => {}
            Err(other) => panic!("expected MissingSession, got {other:?}"),
            Ok(_) => panic!("expected MissingSession, got Ok"),
        }

        // Missing agent.
        match EventEmitter::builder(store)
            .session(SessionId::from_string("sess"))
            .build()
        {
            Err(BuildError::MissingAgent) => {}
            Err(other) => panic!("expected MissingAgent, got {other:?}"),
            Ok(_) => panic!("expected MissingAgent, got Ok"),
        }
    }
}
