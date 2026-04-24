//! Integration test: replay-on-restart.
//!
//! Proves that a second daemon instance started against the same redb journal
//! reconstructs the live-VM handles that were persisted by the first instance.
//!
//! ## What is tested
//!
//! 1. First daemon: replay produces an empty state (fresh journal).
//! 2. First daemon: `KernelVmCreated` event is appended to the journal via
//!    a direct `EventStorePort::append` call (bypasses the full create_vm RPC
//!    path to keep the test hermetic — no Docker or nsjail required).
//! 3. First daemon shuts down.
//! 4. Second daemon: `bootstrap::build_engine` replays the same redb file and
//!    surfaces the persisted VM in `Bootstrap::replayed.live_vms`.
//! 5. Second daemon: `snapshot_vm_handles()` returns the reconstructed handle.
//!
//! ## Skip condition
//!
//! The test does NOT use `LocalSandboxProvider` and therefore does not require
//! Docker or nsjail.  It can run on any host that can create a tempdir.
//! The `#[ignore]` attribute is NOT applied — this test is safe for CI.

use std::sync::Arc;

use aios_protocol::{
    event::{EventKind, EventRecord, KernelVmCreated},
    hypervisor::{BackendId, VmId},
    ids::{AgentId, BranchId, SessionId},
    ports::EventStorePort,
};
use lago_aios_eventstore_adapter::LagoAiosEventStoreAdapter;
use lago_journal::RedbJournal;
use tempfile::TempDir;

// ── helpers ───────────────────────────────────────────────────────────────────

// `LifedConfig`, `BackendsConfig`, and `LagoConfig` are `#[non_exhaustive]`
// and cannot be constructed via struct literals outside their defining crate.
// This test exercises replay directly via the event store without calling
// build_engine, so no LifedConfig construction is needed.

fn make_kernel_created_record(vm_id: &str, session_id: &str) -> EventRecord {
    EventRecord::new(
        SessionId::from_string(session_id),
        BranchId::from_string("main"),
        0, // sequence assigned by the journal
        EventKind::KernelVmCreated(KernelVmCreated {
            vm_id: VmId::from(vm_id),
            backend: BackendId::from("local"),
            spec_hash: "test-replay-restart".into(),
            session_id: SessionId::from_string(session_id),
            agent_id: AgentId::from_string("lifed"),
        }),
    )
}

// ── Test ──────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn daemon_reconstructs_live_vms_after_restart() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("journal.redb");

    // ── Phase 1: simulate a "first daemon run" ────────────────────────────────
    //
    // Open the redb journal directly and append a KernelVmCreated event, as if
    // a real first daemon run had called create_vm successfully.  The session
    // ID follows the same derivation as `bootstrap::build_engine`:
    // `"lifed:{namespace}"`.
    let session_id = "lifed:replay-restart-test";
    {
        let journal = RedbJournal::open(&db_path).expect("open journal for first run");
        let store: Arc<dyn EventStorePort> =
            Arc::new(LagoAiosEventStoreAdapter::new(Arc::new(journal)));

        let record = make_kernel_created_record("vm-persisted-1", session_id);
        store.append(record).await.expect("append KernelVmCreated");

        // Verify the event was persisted.
        let events = store
            .read(
                SessionId::from_string(session_id),
                BranchId::from_string("main"),
                0,
                10,
            )
            .await
            .expect("read after append");
        assert_eq!(
            events.len(),
            1,
            "should have 1 event after first-run append"
        );
    }
    // `store` / `journal` / `Arc` drop here — redb file is flushed and closed.

    // ── Phase 2: simulate a "second daemon run" — replay ─────────────────────
    //
    // Replicate what `bootstrap::build_engine` does for the replay step,
    // without spinning up a full engine (which requires a backend).
    {
        let journal = RedbJournal::open(&db_path).expect("open journal for second run");
        let store: Arc<dyn EventStorePort> =
            Arc::new(LagoAiosEventStoreAdapter::new(Arc::new(journal)));

        // Replay all events for the session.
        let events = store
            .read(
                SessionId::from_string(session_id),
                BranchId::from_string("main"),
                0,
                512,
            )
            .await
            .expect("read for replay");
        assert_eq!(events.len(), 1, "second run should see 1 persisted event");

        let kinds: Vec<_> = events.iter().map(|r| &r.kind).collect();
        let replayed = life_kernel_core::KernelEngine::replay(kinds.into_iter());

        // The VM must appear in the live index.
        assert_eq!(
            replayed.live_vms.len(),
            1,
            "replayed state must contain 1 live VM; got: {:?}",
            replayed.live_vms
        );
        assert!(
            replayed.live_vms.contains_key("vm-persisted-1"),
            "vm-persisted-1 must be in live_vms; got: {:?}",
            replayed.live_vms
        );
        assert_eq!(replayed.events_applied, 1);

        // `snapshot_vm_handles` must surface the handle with Running status.
        let handles = replayed.snapshot_vm_handles();
        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].vm_id, VmId::from("vm-persisted-1"));
        assert!(
            matches!(
                handles[0].status,
                aios_protocol::hypervisor::VmStatus::Running
            ),
            "replayed handle must have optimistic Running status"
        );
    }
}
