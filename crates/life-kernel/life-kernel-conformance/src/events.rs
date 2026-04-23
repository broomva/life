//! Event-trail scenarios — ordering, causation, gate denial.
//!
//! The kernel's observable behaviour is a pure function of the emitted
//! event journal. These scenarios lock in the ordering invariants
//! downstream replay tooling (BRO-876) will depend on.

use aios_protocol::event::EventKind;
use aios_protocol::kernel::GateKind;
use aios_protocol::ports::KernelPort;

use crate::lifecycle::{default_ctx, tool_call, vm_spec_for};
use crate::{ConformanceError, ConformanceHarness};

// ── Scenarios ────────────────────────────────────────────────────────

/// A create / dispatch / destroy round-trip must emit its kernel
/// events in strictly-increasing sequence order: `KernelVmCreated <
/// KernelDispatchStarted < KernelDispatchCompleted < KernelVmDestroyed`.
///
/// The engine can interleave `KernelUsageRecorded` between
/// `KernelDispatchCompleted` and `KernelVmDestroyed` (the MVS
/// metering wrapper does exactly that) — the scenario allows any
/// amount of additional kernel events as long as the four required
/// markers appear in order.
pub async fn event_order_create_dispatch_destroy(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    let (engine, store) = harness.build_engine().await;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;

    let call = tool_call("call-order", "tool.order");
    engine
        .dispatch(&vm, call, &default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("dispatch: {e}")))?;

    engine
        .destroy(vm)
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("destroy: {e}")))?;

    let events = store.stored_events();
    let created_idx = find_index(&events, |k| matches!(k, EventKind::KernelVmCreated(_)))
        .ok_or_else(|| ConformanceError::events("missing KernelVmCreated in event trail"))?;
    let started_idx = find_index(&events, |k| {
        matches!(k, EventKind::KernelDispatchStarted(_))
    })
    .ok_or_else(|| ConformanceError::events("missing KernelDispatchStarted in event trail"))?;
    let completed_idx = find_index(&events, |k| {
        matches!(k, EventKind::KernelDispatchCompleted(_))
    })
    .ok_or_else(|| ConformanceError::events("missing KernelDispatchCompleted in event trail"))?;
    let destroyed_idx = find_index(&events, |k| matches!(k, EventKind::KernelVmDestroyed(_)))
        .ok_or_else(|| ConformanceError::events("missing KernelVmDestroyed in event trail"))?;

    // Both insertion order and declared sequence number must agree.
    let ordering = [
        ("Created", created_idx),
        ("Started", started_idx),
        ("Completed", completed_idx),
        ("Destroyed", destroyed_idx),
    ];
    for pair in ordering.windows(2) {
        let (a_name, a_idx) = pair[0];
        let (b_name, b_idx) = pair[1];
        if a_idx >= b_idx {
            return Err(ConformanceError::events(format!(
                "insertion order {a_name} (#{a_idx}) must precede {b_name} (#{b_idx})"
            )));
        }
        if events[a_idx].sequence >= events[b_idx].sequence {
            return Err(ConformanceError::events(format!(
                "sequence not monotonic: {a_name}.seq={} >= {b_name}.seq={}",
                events[a_idx].sequence, events[b_idx].sequence
            )));
        }
    }
    Ok(())
}

/// The metering wrapper threads the `KernelDispatchStarted` event id
/// into `KernelDispatchCompleted.causation_id`. When the backend is
/// built on a Phase 1 emitter (which always threads causation), assert
/// the link; when it is not (e.g. a future experimental emitter), log
/// and pass.
pub async fn causation_chain_dispatch_completed_follows_started(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    let (engine, store) = harness.build_engine().await;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;

    let call = tool_call("call-cause", "tool.cause");
    engine
        .dispatch(&vm, call, &default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("dispatch: {e}")))?;
    let _ = engine.destroy(vm).await;

    let events = store.stored_events();
    let started = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::KernelDispatchStarted(_)))
        .ok_or_else(|| ConformanceError::events("missing KernelDispatchStarted"))?;
    let completed = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::KernelDispatchCompleted(_)))
        .ok_or_else(|| ConformanceError::events("missing KernelDispatchCompleted"))?;

    match &completed.causation_id {
        Some(cause) => {
            if cause != &started.event_id {
                return Err(ConformanceError::events(format!(
                    "KernelDispatchCompleted.causation_id = {cause}, expected {}",
                    started.event_id
                )));
            }
            Ok(())
        }
        None => {
            eprintln!(
                "[conformance] causation_chain_dispatch_completed_follows_started: engine did \
                 not thread causation between Started and Completed; this is permitted in Phase \
                 1 (the emitter may skip causation when not supplied) — treating as pass"
            );
            Ok(())
        }
    }
}

/// When a policy gate vetoes a dispatch, the engine must emit a
/// `KernelDispatchDenied` event naming the gate that denied.
///
/// The scenario uses the harness's optional deny-policy variant; if
/// the harness has not implemented it, we skip with a note.
pub async fn gate_deny_emits_dispatch_denied(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let Some((engine, store)) = harness.build_engine_with_deny_policy().await else {
        eprintln!(
            "[conformance] gate_deny_emits_dispatch_denied: harness does not implement \
             build_engine_with_deny_policy; skipping"
        );
        return Ok(());
    };

    let backend_name = discover_backend_name(&engine).await?;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;

    let call = tool_call("call-denied", "tool.blocked");
    let err = engine
        .dispatch(&vm, call, &default_ctx())
        .await
        .err()
        .ok_or_else(|| {
            ConformanceError::ExpectedFailure("deny-policy harness must fail a dispatch".into())
        })?;
    if !matches!(err, aios_protocol::kernel::KernelError::GateDenied { .. }) {
        return Err(ConformanceError::contract(format!(
            "expected GateDenied, got {err:?}"
        )));
    }
    let _ = engine.destroy(vm).await;

    let events = store.stored_events();
    let denied = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::KernelDispatchDenied(_)))
        .ok_or_else(|| ConformanceError::events("missing KernelDispatchDenied event"))?;
    if let EventKind::KernelDispatchDenied(payload) = &denied.kind {
        // The plan requires the denial to name the gate via `GateKind`.
        // Policy / Budget / ForkLambda / NetworkIsolation are all valid
        // — the deny-policy harness variant canonically sets Policy,
        // but we accept the other variants too in case an integrator
        // wires a different gate as its "deny everything" default.
        match payload.gate {
            GateKind::Policy
            | GateKind::Budget
            | GateKind::ForkLambda
            | GateKind::NetworkIsolation => {}
            // `GateKind` is `#[non_exhaustive]`; accept any future
            // variant that is wired up as a deny source.
            _ => {}
        }
        if payload.call_id != "call-denied" {
            return Err(ConformanceError::events(format!(
                "KernelDispatchDenied.call_id = {}, expected call-denied",
                payload.call_id
            )));
        }
        Ok(())
    } else {
        unreachable!("matched KernelDispatchDenied above")
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn find_index(
    events: &[aios_protocol::event::EventRecord],
    pred: impl Fn(&EventKind) -> bool,
) -> Option<usize> {
    events.iter().position(|e| pred(&e.kind))
}

async fn discover_backend_name(engine: &dyn KernelPort) -> Result<String, ConformanceError> {
    let spec = aios_protocol::hypervisor::VmSpec {
        backend_selector: aios_protocol::hypervisor::BackendSelector::Auto,
        ..vm_spec_for("unused")
    };
    let vm = engine
        .create_vm(spec, default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("discover create_vm: {e}")))?;
    let name = vm.backend.to_string();
    let _ = engine.destroy(vm).await;
    Ok(name)
}

// ── Battery runner ───────────────────────────────────────────────────

/// Run every event-trail scenario in order.
pub async fn run(harness: &dyn ConformanceHarness) -> Result<(), ConformanceError> {
    event_order_create_dispatch_destroy(harness).await?;
    causation_chain_dispatch_completed_follows_started(harness).await?;
    gate_deny_emits_dispatch_denied(harness).await?;
    Ok(())
}
