//! Build a running `KernelEngine` from a `LifedConfig`.
//!
//! Handles event store instantiation (in-memory via TempDir-backed RedbJournal
//! for MVS, or an explicit on-disk redb path), backend registration, gate
//! chain composition, and a one-shot replay of the configured session's
//! events into a `ReplayedState` the caller can use to seed its live-VM index.

use std::sync::Arc;

use aios_protocol::error::KernelResult as LegacyKernelResult;
use aios_protocol::ids::{AgentId, ApprovalId, BranchId, SessionId};
use aios_protocol::policy::Capability;
use aios_protocol::ports::{
    ApprovalPort, ApprovalRequest, ApprovalResolution, ApprovalTicket, EventStorePort,
    PolicyGateDecision, PolicyGatePort,
};
use async_trait::async_trait;
use chrono::Utc;
use lago_aios_eventstore_adapter::LagoAiosEventStoreAdapter;
use lago_journal::RedbJournal;
use life_kernel_core::{KernelEngine, ReplayedState};
use life_kernel_gate::{NoOpBudgetGate, NoOpNetworkIsolation, StaticPolicyGate};
use tempfile::TempDir;
use tracing::warn;

use crate::config::{LagoConfig, LagoStoreKind, LifedConfig};
use crate::error::{LifedError, LifedResult};

// ── Allow-all policy + approval stubs ───────────────────────────────────────
//
// Mirrors the shape of `StubPolicyGate` / `StubApprovalPort` from
// `life-kernel-gate/src/policy.rs` tests. They are private to that module, so
// we redeclare them here scoped to bootstrap, with a comment pointing at the
// source.

/// Allow-all [`PolicyGatePort`] — returns an all-allowed decision for every
/// session. Phase 2 MVS only; real policy lands in Phase 4.
///
/// Shape mirrors the `StubPolicyGate` in
/// `life-kernel-gate/src/policy.rs` tests (allow-everything variant).
struct AllowAllPolicyGate;

#[async_trait]
impl PolicyGatePort for AllowAllPolicyGate {
    async fn evaluate(
        &self,
        _session_id: SessionId,
        _requested: Vec<Capability>,
    ) -> LegacyKernelResult<PolicyGateDecision> {
        Ok(PolicyGateDecision {
            allowed: Vec::new(),
            requires_approval: Vec::new(),
            denied: Vec::new(),
        })
    }
}

/// Approval port that always enqueues successfully and never blocks.
///
/// Phase 2 MVS only. Shape mirrors `StubApprovalPort` from
/// `life-kernel-gate/src/policy.rs` tests.
struct AllowAllApprovalPort;

#[async_trait]
impl ApprovalPort for AllowAllApprovalPort {
    async fn enqueue(&self, request: ApprovalRequest) -> LegacyKernelResult<ApprovalTicket> {
        Ok(ApprovalTicket {
            approval_id: ApprovalId::from_string("auto-approved"),
            session_id: request.session_id,
            call_id: request.call_id,
            tool_name: request.tool_name,
            capability: request.capability,
            reason: request.reason,
            created_at: Utc::now(),
        })
    }

    async fn list_pending(
        &self,
        _session_id: SessionId,
    ) -> LegacyKernelResult<Vec<ApprovalTicket>> {
        Ok(Vec::new())
    }

    async fn resolve(
        &self,
        approval_id: ApprovalId,
        approved: bool,
        actor: String,
    ) -> LegacyKernelResult<ApprovalResolution> {
        Ok(ApprovalResolution {
            approval_id,
            approved,
            actor,
            resolved_at: Utc::now(),
        })
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Complete daemon bootstrap result — the engine + the replayed state ready
/// to seed downstream server state, plus an optional `TempDir` owning the
/// in-memory store's filesystem backing (held so it lives as long as the
/// daemon).
#[allow(missing_debug_implementations)] // KernelEngine and TempDir are not Debug
pub struct Bootstrap {
    /// The assembled `KernelEngine`, wrapped in `Arc` so it can be shared
    /// across the tonic service (BRO-900) and any other consumers.
    pub engine: Arc<KernelEngine>,
    /// State reconstructed from replaying all existing `kernel.*` events for
    /// the configured session. Empty on first start.
    pub replayed: ReplayedState,
    /// The `EventStorePort` the engine was built against. Shared so BRO-900
    /// can subscribe to the live stream without going through the engine.
    pub event_store: Arc<dyn EventStorePort>,
    /// Session ID the engine is attributed to.
    pub session_id: SessionId,
    /// Branch ID the engine emits events on.
    pub branch_id: BranchId,
    /// Holds the `TempDir` backing the Lago journal for `LagoStoreKind::InMemory`
    /// so the temporary redb file outlives the bootstrap call.
    /// `None` when an explicit `Redb { path }` is configured.
    pub _lago_tempdir: Option<TempDir>,
}

/// Assemble a [`Bootstrap`] from a validated [`LifedConfig`].
///
/// Steps:
/// 1. Build the event store (redb or in-memory-backed-by-tempdir).
/// 2. Derive session/branch/agent IDs from the config namespace.
/// 3. Replay existing events to reconstruct live-VM state.
/// 4. Compose gates (NoOp budget, NoOp network, StaticPolicy with allow-all
///    stubs — real gates land in Phase 4).
/// 5. Register backends per `cfg.backends` and build the engine.
pub async fn build_engine(cfg: &LifedConfig) -> LifedResult<Bootstrap> {
    // Validate first — `load()` already runs this, but guard against callers
    // that construct `LifedConfig` by hand without going through `load()`.
    cfg.validate()?;

    // 1. Event store.
    let (event_store, _lago_tempdir) = build_event_store(&cfg.lago).await?;

    // 2. IDs.
    let session_id = SessionId::from_string(format!("lifed:{}", cfg.lago.namespace));
    let branch_id = BranchId::main();
    let agent_id = AgentId::from_string("lifed");

    // 3. Replay.
    let replayed = replay_from_store(Arc::clone(&event_store), &session_id, &branch_id).await?;

    // 4. Gates.
    let policy_gate: Arc<dyn aios_protocol::budget::BudgetGatePort> = Arc::new(
        StaticPolicyGate::new(Arc::new(AllowAllPolicyGate), Arc::new(AllowAllApprovalPort)),
    );
    let budget_gate: Arc<dyn aios_protocol::budget::BudgetGatePort> =
        Arc::new(NoOpBudgetGate::new());
    let network_gate: Arc<dyn aios_protocol::network_isolation::NetworkIsolationPort> =
        Arc::new(NoOpNetworkIsolation::new());

    // 5. Build engine — register backends from config.
    if !cfg.backends.local && cfg.backends.cube.is_none() && cfg.backends.vercel.is_none() {
        return Err(LifedError::Config(
            "at least one backend must be enabled ([backends] section)".into(),
        ));
    }

    let mut builder = KernelEngine::builder()
        .policy_gate(policy_gate)
        .budget_gate(budget_gate)
        .network_isolation(network_gate)
        .event_store(Arc::clone(&event_store))
        .session(session_id.clone(), agent_id);

    // Local backend.
    if cfg.backends.local {
        let local = arcan_provider_local::LocalSandboxProvider::from_env()
            .map_err(|e| LifedError::BackendInit(format!("arcan-provider-local: {e}")))?;
        builder = builder.register_backend(Arc::new(local)).await;
    }

    // Cube backend — placeholder until Phase 3 (BRO-???).
    if let Some(_cube_cfg) = &cfg.backends.cube {
        unimplemented!(
            "Cube backend unavailable until BRO-??? (Phase 3). \
             Remove [backends.cube] from your config to use the local backend."
        );
    }

    // Vercel backend — placeholder until Phase 4 (BRO-???).
    if let Some(_vercel_cfg) = &cfg.backends.vercel {
        unimplemented!(
            "Vercel backend unavailable until BRO-??? (Phase 4). \
             Remove [backends.vercel] from your config to use the local backend."
        );
    }

    let engine = builder
        .build()
        .await
        .map_err(|e| LifedError::BackendInit(format!("KernelEngine::build: {e}")))?;

    Ok(Bootstrap {
        engine: Arc::new(engine),
        replayed,
        event_store,
        session_id,
        branch_id,
        _lago_tempdir,
    })
}

// ── Private helpers ──────────────────────────────────────────────────────────

/// Instantiate the Lago event store from the `[lago]` config section.
///
/// Returns the store plus an optional `TempDir` that must live as long as
/// the daemon for the `InMemory` variant.
async fn build_event_store(
    cfg: &LagoConfig,
) -> LifedResult<(Arc<dyn EventStorePort>, Option<TempDir>)> {
    match &cfg.store {
        LagoStoreKind::InMemory => {
            let dir = TempDir::new().map_err(|e| {
                LifedError::BackendInit(format!("tempdir for in-memory store: {e}"))
            })?;
            let db_path = dir.path().join("journal.redb");
            let journal = RedbJournal::open(&db_path).map_err(|e| {
                LifedError::BackendInit(format!("RedbJournal::open({db_path:?}): {e}"))
            })?;
            let adapter = LagoAiosEventStoreAdapter::new(Arc::new(journal));
            Ok((Arc::new(adapter) as Arc<dyn EventStorePort>, Some(dir)))
        }
        LagoStoreKind::Redb { path } => {
            let journal = RedbJournal::open(path).map_err(|e| {
                LifedError::BackendInit(format!("RedbJournal::open({path:?}): {e}"))
            })?;
            let adapter = LagoAiosEventStoreAdapter::new(Arc::new(journal));
            Ok((Arc::new(adapter) as Arc<dyn EventStorePort>, None))
        }
    }
}

/// Replay all existing `kernel.*` events for the given session/branch into a
/// [`ReplayedState`].
///
/// Reads in pages of 512 until the store returns a short page (end-of-stream).
/// Caps total events at 10 000; if the cap is hit a `WARN` is emitted and
/// the fold continues over the events read so far.
async fn replay_from_store(
    store: Arc<dyn EventStorePort>,
    session_id: &SessionId,
    branch_id: &BranchId,
) -> LifedResult<ReplayedState> {
    const PAGE_SIZE: usize = 512;
    const MAX_EVENTS: usize = 10_000;

    let mut all_records = Vec::new();
    let mut cursor: u64 = 0;

    loop {
        let batch = store
            .read(session_id.clone(), branch_id.clone(), cursor, PAGE_SIZE)
            .await
            .map_err(|e| LifedError::BackendInit(format!("event store read during replay: {e}")))?;

        let batch_len = batch.len();
        for record in &batch {
            // Advance cursor past the last sequence we read.
            if record.sequence >= cursor {
                cursor = record.sequence + 1;
            }
        }
        all_records.extend(batch);

        if all_records.len() >= MAX_EVENTS {
            warn!(
                total = all_records.len(),
                max = MAX_EVENTS,
                "replay hit the 10 000-event safety cap; \
                 folding available events and continuing"
            );
            break;
        }

        if batch_len < PAGE_SIZE {
            // Short page → end of stream.
            break;
        }
    }

    let kinds: Vec<_> = all_records.iter().map(|r| &r.kind).collect();
    Ok(KernelEngine::replay(kinds))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use aios_protocol::event::{EventKind, KernelVmCreated, KernelVmDestroyed};
    use aios_protocol::hypervisor::{BackendId, VmId};
    use aios_protocol::ids::{BranchId, SessionId};

    use crate::config::{BackendsConfig, GatesConfig, LagoConfig, LagoStoreKind, LifedConfig};

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_record(
        session: &str,
        branch: &str,
        kind: EventKind,
    ) -> aios_protocol::event::EventRecord {
        aios_protocol::event::EventRecord::new(
            SessionId::from_string(session),
            BranchId::from_string(branch),
            0, // sequence assigned by the journal
            kind,
        )
    }

    fn kernel_created(vm_id: &str, session: &str) -> EventKind {
        EventKind::KernelVmCreated(KernelVmCreated {
            vm_id: VmId::from(vm_id),
            backend: BackendId::from("local"),
            spec_hash: "test-hash".into(),
            session_id: SessionId::from_string(session),
            agent_id: AgentId::from_string("lifed"),
        })
    }

    fn kernel_destroyed(vm_id: &str) -> EventKind {
        EventKind::KernelVmDestroyed(KernelVmDestroyed {
            vm_id: VmId::from(vm_id),
            reason: "test-teardown".into(),
        })
    }

    // ── build_event_store tests ───────────────────────────────────────────────

    #[tokio::test]
    async fn build_event_store_in_memory_returns_usable_port_and_tempdir() {
        let cfg = LagoConfig {
            namespace: "test-inmem".into(),
            store: LagoStoreKind::InMemory,
        };
        let (store, tempdir) = build_event_store(&cfg).await.unwrap();
        assert!(tempdir.is_some(), "InMemory variant must return a TempDir");

        // Verify the store is functional by appending and reading back a record.
        let record = make_record(
            "sess-inmem",
            "main",
            EventKind::Message {
                role: "user".into(),
                content: "ping".into(),
                model: None,
                token_usage: None,
            },
        );
        let stored = store.append(record).await.unwrap();
        assert!(
            stored.sequence > 0,
            "store must assign a monotonic sequence"
        );

        let events = store
            .read(
                SessionId::from_string("sess-inmem"),
                BranchId::from_string("main"),
                0,
                10,
            )
            .await
            .unwrap();
        assert_eq!(events.len(), 1);

        // TempDir is held alive here; dropping `tempdir` at end-of-scope is fine.
    }

    #[tokio::test]
    async fn build_event_store_redb_uses_explicit_path() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("x.redb");

        let cfg = LagoConfig {
            namespace: "test-redb".into(),
            store: LagoStoreKind::Redb {
                path: db_path.clone(),
            },
        };
        let (store, tempdir) = build_event_store(&cfg).await.unwrap();
        assert!(
            tempdir.is_none(),
            "Redb variant must return None for TempDir"
        );
        assert!(
            db_path.exists(),
            "redb file must be created at the explicit path"
        );

        // Sanity: store is usable.
        let record = make_record(
            "sess-redb",
            "main",
            EventKind::Message {
                role: "user".into(),
                content: "pong".into(),
                model: None,
                token_usage: None,
            },
        );
        store.append(record).await.unwrap();
    }

    // ── replay_from_store tests ───────────────────────────────────────────────

    #[tokio::test]
    async fn replay_from_store_reconstructs_live_vm_after_create_then_destroy() {
        let dir = TempDir::new().unwrap();
        let journal: Arc<dyn lago_core::Journal> =
            Arc::new(RedbJournal::open(dir.path().join("journal.redb")).unwrap());
        let store: Arc<dyn EventStorePort> =
            Arc::new(LagoAiosEventStoreAdapter::new(Arc::clone(&journal)));

        let session = "sess-replay-cd";
        let branch = "main";
        let vm_id = "vm-replay-1";

        // Append: create then destroy the same VM.
        store
            .append(make_record(session, branch, kernel_created(vm_id, session)))
            .await
            .unwrap();
        store
            .append(make_record(session, branch, kernel_destroyed(vm_id)))
            .await
            .unwrap();

        let replayed = replay_from_store(
            Arc::clone(&store),
            &SessionId::from_string(session),
            &BranchId::from_string(branch),
        )
        .await
        .unwrap();

        // The VM was created and then destroyed — live_vms must be empty.
        assert!(
            replayed.live_vms.is_empty(),
            "destroyed VM must not appear in live_vms; got: {:?}",
            replayed.live_vms
        );
        // Both events must have been folded.
        assert_eq!(replayed.events_applied, 2);
    }

    // ── build_engine tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn build_engine_fails_with_no_backends_enabled() {
        // Construct a LifedConfig that has no backends enabled.
        // We do this by bypassing `validate()` — the point of this test is to
        // verify that `build_engine` *also* surfaces the error if called
        // without prior `load()`.
        let cfg = LifedConfig {
            backends: BackendsConfig {
                local: false,
                cube: None,
                vercel: None,
            },
            gates: GatesConfig::default(),
            lago: LagoConfig {
                namespace: "no-backend-test".into(),
                store: LagoStoreKind::InMemory,
            },
            ..Default::default()
        };

        match build_engine(&cfg).await {
            Err(LifedError::Config(msg)) => {
                assert!(
                    msg.contains("at least one backend"),
                    "expected 'at least one backend' in error message, got: {msg}"
                );
            }
            Err(other) => panic!("expected LifedError::Config, got: {other:?}"),
            Ok(_) => panic!("expected an error when no backends are enabled, but got Ok"),
        }
    }
}
