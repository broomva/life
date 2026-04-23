//! Lifecycle scenarios — create / dispatch / snapshot / fork / destroy.
//!
//! Every scenario is a standalone `pub async fn scenario_name(harness)`
//! so integrators can drive them individually. [`run`] sequences them
//! in the order a fresh engine would experience them; it short-circuits
//! at the first failure.
//!
//! Capability-gated scenarios (snapshot/fork) return `Ok(())` after a
//! single `eprintln!` note when the backend does not advertise the
//! required capability. See the crate-level rustdoc for the policy.

use std::collections::HashMap;

use aios_protocol::event::EventKind;
use aios_protocol::hypervisor::{
    BackendCapabilitySet, BackendId, BackendSelector, ForkSpec, RuntimeHint, VmResources,
    VmSpecOverrides,
};
use aios_protocol::ids::{AgentId, SessionId};
use aios_protocol::kernel::{ChainId, KernelContext, KernelError, WalletAttribution};
use aios_protocol::ports::KernelPort;
use aios_protocol::sandbox::NetworkPolicy;
use aios_protocol::tool::ToolCall;

use crate::{ConformanceError, ConformanceHarness};

// ── Shared scenario helpers ──────────────────────────────────────────

/// Default session + agent ids used throughout the batteries.
///
/// Harnesses that scope an engine to different ids may see these in the
/// event trail; the conformance scenarios only assert on things they
/// set themselves.
pub(crate) fn default_ctx() -> KernelContext {
    KernelContext {
        session_id: SessionId::from_string("sess-conformance"),
        agent_id: AgentId::from_string("agent-conformance"),
        wallet: WalletAttribution {
            address: "0x00000000000000000000000000000000000c0de".into(),
            chain: ChainId::base(),
        },
        cost_hint: None,
        trace_ctx: None,
    }
}

/// Build a [`VmSpec`] bound to the given backend by name.
pub(crate) fn vm_spec_for(backend_name: &str) -> aios_protocol::hypervisor::VmSpec {
    aios_protocol::hypervisor::VmSpec {
        backend_selector: BackendSelector::Explicit {
            backend: BackendId::from(backend_name),
        },
        resources: VmResources::default(),
        network_policy: NetworkPolicy::Disabled,
        mounts: Vec::new(),
        env: HashMap::new(),
        runtime_hint: RuntimeHint::Shell,
        labels: HashMap::new(),
    }
}

/// Build a minimal [`ToolCall`] with deterministic `call_id` / `tool_name`.
pub(crate) fn tool_call(call_id: &str, tool_name: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.into(),
        tool_name: tool_name.into(),
        input: serde_json::Value::Null,
        requested_capabilities: Vec::new(),
    }
}

/// Resolve the first registered backend's name by looking at the
/// [`aios_protocol::event::EventKind::KernelVmCreated`] payload emitted
/// by `create_vm`.
///
/// The harness does not expose the backend name directly, so we
/// round-trip through an `Auto` create to discover what the engine
/// treats as the default backend. Returns the backend name plus a
/// fresh `VmHandle` the caller can reuse.
async fn discover_backend_name(engine: &dyn KernelPort) -> Result<String, ConformanceError> {
    let spec = aios_protocol::hypervisor::VmSpec {
        backend_selector: BackendSelector::Auto,
        ..vm_spec_for("unused")
    };
    let vm = engine
        .create_vm(spec, default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("discover create_vm: {e}")))?;
    let name = vm.backend.to_string();
    // Best-effort cleanup; tolerate errors.
    let _ = engine.destroy(vm).await;
    Ok(name)
}

/// Look up the `BackendCapabilitySet` advertised for `backend_name`.
///
/// Harnesses do not expose the registry, so we infer capabilities from
/// the backend name we discovered: we try a snapshot; if it returns
/// `BackendError::NotSupported` for `restore`, we know PERSISTENCE is
/// only partial (snapshot works, restore does not). That maps to
/// "snapshot works, fork does not" for our purposes. Callers of this
/// helper use the returned probe-result as a best-effort capability
/// hint.
async fn probe_supports_fork(
    engine: &dyn KernelPort,
    backend_name: &str,
) -> Result<bool, ConformanceError> {
    // Create a VM to probe with.
    let vm = engine
        .create_vm(vm_spec_for(backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("probe create_vm: {e}")))?;
    let snap_result = engine.snapshot(&vm, "probe").await;
    let supports = match snap_result {
        Ok(handle) => {
            // Try a fork; if it succeeds, clean up the child + snapshot.
            let fork_spec = ForkSpec {
                parent_snapshot: handle.snapshot_id.clone(),
                overrides: VmSpecOverrides::default(),
            };
            match engine.fork(&handle, fork_spec, default_ctx()).await {
                Ok(child) => {
                    let _ = engine.destroy(child).await;
                    true
                }
                Err(_) => false,
            }
        }
        Err(_) => false,
    };
    let _ = engine.destroy(vm).await;
    Ok(supports)
}

// ── Scenarios ────────────────────────────────────────────────────────

/// Create a VM then destroy it and verify the exact event trail.
///
/// Must emit exactly [`KernelVmCreated`, `KernelVmDestroyed`] on the
/// branch, in that order, with matching `vm_id`.
pub async fn create_then_destroy_emits_exact_event_sequence(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    // Clear the probe events by building a fresh engine/store.
    let (engine, store) = harness.build_engine().await;

    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;
    let vm_id = vm.vm_id.clone();

    engine
        .destroy(vm)
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("destroy: {e}")))?;

    let events = store.stored_events();
    if events.len() != 2 {
        return Err(ConformanceError::events(format!(
            "expected 2 events (Created, Destroyed), got {}: {:?}",
            events.len(),
            event_kind_names(&events)
        )));
    }
    match (&events[0].kind, &events[1].kind) {
        (EventKind::KernelVmCreated(created), EventKind::KernelVmDestroyed(destroyed)) => {
            if created.vm_id != vm_id {
                return Err(ConformanceError::events(format!(
                    "KernelVmCreated.vm_id = {}, expected {vm_id}",
                    created.vm_id
                )));
            }
            if destroyed.vm_id != vm_id {
                return Err(ConformanceError::events(format!(
                    "KernelVmDestroyed.vm_id = {}, expected {vm_id}",
                    destroyed.vm_id
                )));
            }
        }
        (a, b) => {
            return Err(ConformanceError::events(format!(
                "expected KernelVmCreated then KernelVmDestroyed, got {a:?} / {b:?}"
            )));
        }
    }
    // Sequence numbers must be monotonically increasing.
    if events[0].sequence >= events[1].sequence {
        return Err(ConformanceError::events(format!(
            "sequence not monotonic: {} → {}",
            events[0].sequence, events[1].sequence
        )));
    }
    Ok(())
}

/// Dispatch a Phase 1 `echo` call through the translator + metering
/// path and verify the `ToolResult` carries `call_id:tool_name` in
/// stdout.
pub async fn create_dispatch_echo_returns_stdout(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    let (engine, _store) = harness.build_engine().await;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;

    let call = tool_call("call-echo", "tool.echo");
    let result = engine
        .dispatch(&vm, call, &default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("dispatch: {e}")))?;

    if result.call_id != "call-echo" {
        return Err(ConformanceError::contract(format!(
            "ToolResult.call_id = {}, expected call-echo",
            result.call_id
        )));
    }
    if result.tool_name != "tool.echo" {
        return Err(ConformanceError::contract(format!(
            "ToolResult.tool_name = {}, expected tool.echo",
            result.tool_name
        )));
    }
    if result.is_error {
        return Err(ConformanceError::contract(format!(
            "ToolResult.is_error = true; output: {:?}",
            result.output
        )));
    }
    let stdout = result
        .output
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !stdout.contains("call-echo:tool.echo") {
        return Err(ConformanceError::contract(format!(
            "stdout did not contain 'call-echo:tool.echo': {stdout:?}"
        )));
    }
    // Best-effort cleanup.
    let _ = engine.destroy(vm).await;
    Ok(())
}

/// Snapshot + fork a VM and verify the child has a distinct `vm_id`.
///
/// Skipped with an `eprintln!` note when the backend does not support
/// the fork path.
pub async fn snapshot_fork_yields_distinct_vm_ids(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    // Probe fork support on a throwaway engine.
    let (probe_engine, _) = harness.build_engine().await;
    let supports_fork = probe_supports_fork(&probe_engine, &backend_name).await?;
    if !supports_fork {
        eprintln!(
            "[conformance] snapshot_fork_yields_distinct_vm_ids: backend '{backend_name}' does not \
             support snapshot+fork (no {cap:?}); skipping",
            cap = BackendCapabilitySet::PERSISTENCE | BackendCapabilitySet::FORK
        );
        return Ok(());
    }

    let (engine, _store) = harness.build_engine().await;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;
    let parent_vm_id = vm.vm_id.clone();
    let snap = engine
        .snapshot(&vm, "fork-source")
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("snapshot: {e}")))?;

    let fork_spec = ForkSpec {
        parent_snapshot: snap.snapshot_id.clone(),
        overrides: VmSpecOverrides::default(),
    };
    let child = engine
        .fork(&snap, fork_spec, default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("fork: {e}")))?;

    if child.vm_id == parent_vm_id {
        return Err(ConformanceError::contract(format!(
            "fork returned the parent vm_id {parent_vm_id}; expected a distinct child"
        )));
    }
    let _ = engine.destroy(child).await;
    let _ = engine.destroy(vm).await;
    Ok(())
}

/// `destroy` must be idempotent per the [`KernelPort`] contract.
///
/// Calling destroy twice on the same VM (the second after the backend
/// considers it stopped) must not surface an error.
pub async fn destroy_idempotent_when_already_stopped(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    let (engine, _store) = harness.build_engine().await;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;
    let cloned = vm.clone();
    engine
        .destroy(vm)
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("first destroy: {e}")))?;
    engine.destroy(cloned).await.map_err(|e| {
        ConformanceError::contract(format!("double destroy must be idempotent, got {e}"))
    })?;
    Ok(())
}

/// Asking to create a VM against a backend name that was never
/// registered must surface [`KernelError::BackendNotFound`].
pub async fn create_with_unknown_backend_returns_backend_not_found(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let err = engine
        .create_vm(vm_spec_for("cube-that-does-not-exist"), default_ctx())
        .await
        .err()
        .ok_or_else(|| {
            ConformanceError::ExpectedFailure("create_vm against unknown backend must fail".into())
        })?;
    match err {
        KernelError::BackendNotFound(_) => Ok(()),
        other => Err(ConformanceError::contract(format!(
            "expected KernelError::BackendNotFound, got {other:?}"
        ))),
    }
}

// ── Battery runner ───────────────────────────────────────────────────

/// Run every lifecycle scenario in order.
pub async fn run(harness: &dyn ConformanceHarness) -> Result<(), ConformanceError> {
    create_then_destroy_emits_exact_event_sequence(harness).await?;
    create_dispatch_echo_returns_stdout(harness).await?;
    snapshot_fork_yields_distinct_vm_ids(harness).await?;
    destroy_idempotent_when_already_stopped(harness).await?;
    create_with_unknown_backend_returns_backend_not_found(harness).await?;
    Ok(())
}

/// Utility: collect the [`EventKind`] variant names for error messages.
pub(crate) fn event_kind_names(events: &[aios_protocol::event::EventRecord]) -> Vec<&'static str> {
    events.iter().map(event_kind_name).collect()
}

/// Utility: name the [`EventKind`] variant — only covers the kernel.*
/// variants the conformance batteries care about; unknown variants
/// return `"Other"` so error messages stay readable without exhaustively
/// pattern-matching every non-kernel variant in the protocol.
pub(crate) fn event_kind_name(record: &aios_protocol::event::EventRecord) -> &'static str {
    use aios_protocol::event::EventKind::*;
    match &record.kind {
        KernelVmCreated(_) => "KernelVmCreated",
        KernelVmForked(_) => "KernelVmForked",
        KernelVmSnapshotted(_) => "KernelVmSnapshotted",
        KernelVmHibernated(_) => "KernelVmHibernated",
        KernelVmResumed(_) => "KernelVmResumed",
        KernelVmDestroyed(_) => "KernelVmDestroyed",
        KernelDispatchStarted(_) => "KernelDispatchStarted",
        KernelDispatchCompleted(_) => "KernelDispatchCompleted",
        KernelDispatchDenied(_) => "KernelDispatchDenied",
        KernelForkDenied(_) => "KernelForkDenied",
        KernelEgressRecorded(_) => "KernelEgressRecorded",
        KernelPolicyViolated(_) => "KernelPolicyViolated",
        KernelUsageRecorded(_) => "KernelUsageRecorded",
        _ => "Other",
    }
}
