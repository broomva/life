//! Metering scenarios — `ResourceUsage`, confidence, and wallet
//! attribution on `KernelUsageRecorded` events.
//!
//! These scenarios walk the three canonical metering events
//! (`KernelDispatchStarted` → `KernelDispatchCompleted` →
//! `KernelUsageRecorded`) and assert the invariants every backend must
//! honour: the wrapper measures *something* for every dispatch, confidence
//! is a concrete signal rather than `Unknown`, and the recorded usage is
//! attributed to the wallet that initiated the call.

use aios_protocol::budget::UsageConfidence;
use aios_protocol::event::EventKind;
use aios_protocol::ports::KernelPort;

use crate::lifecycle::{default_ctx, tool_call, vm_spec_for};
use crate::{ConformanceError, ConformanceHarness};

// ── Scenarios ────────────────────────────────────────────────────────

/// A non-trivial dispatch must populate `duration_ms > 0` on the
/// returned `ResourceUsage`.
///
/// The MVS metering wrapper prefers the backend's self-reported
/// duration when non-zero and falls back to its own wall clock, so
/// even the fastest echo-backed path should surface at least `1`.
pub async fn dispatch_populates_duration_ms(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    let (engine, _store) = harness.build_engine().await;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;

    let call = tool_call("call-duration", "tool.duration");
    let result = engine
        .dispatch(&vm, call, &default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("dispatch: {e}")))?;

    let usage = result
        .usage
        .ok_or_else(|| ConformanceError::metering("dispatch did not populate usage"))?;
    if usage.duration_ms == 0 {
        return Err(ConformanceError::metering(format!(
            "usage.duration_ms = 0 on successful dispatch; expected a non-zero measurement — \
             full usage: {usage:?}"
        )));
    }
    let _ = engine.destroy(vm).await;
    Ok(())
}

/// Confidence must be either `Measured` or `Estimated` — never
/// `Unknown` — for a successful dispatch. `Unknown` is reserved for
/// fields a backend cannot report at all, not whole dispatches.
pub async fn dispatch_confidence_is_measured_or_estimated(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    let (engine, _store) = harness.build_engine().await;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;

    let call = tool_call("call-confidence", "tool.confidence");
    let result = engine
        .dispatch(&vm, call, &default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("dispatch: {e}")))?;

    let usage = result
        .usage
        .ok_or_else(|| ConformanceError::metering("dispatch did not populate usage"))?;
    match usage.confidence {
        UsageConfidence::Measured | UsageConfidence::Estimated => {}
        UsageConfidence::Unknown => {
            return Err(ConformanceError::metering(
                "usage.confidence = Unknown on a successful dispatch; expected \
                 Measured or Estimated",
            ));
        }
        // `UsageConfidence` is `#[non_exhaustive]`; any future variant
        // added by the protocol must still be one of the concrete
        // (non-Unknown) signals for a successful dispatch.
        _ => {}
    }
    let _ = engine.destroy(vm).await;
    Ok(())
}

/// The `KernelUsageRecorded` event must attribute the dispatch to the
/// wallet carried by the context that initiated it.
pub async fn usage_recorded_event_attributes_wallet(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    let (engine, _store) = harness.build_engine().await;
    let backend_name = discover_backend_name(&engine).await?;

    let (engine, store) = harness.build_engine().await;
    let vm = engine
        .create_vm(vm_spec_for(&backend_name), default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("create_vm: {e}")))?;

    let call = tool_call("call-wallet", "tool.wallet");
    engine
        .dispatch(&vm, call, &default_ctx())
        .await
        .map_err(|e| ConformanceError::UnexpectedError(format!("dispatch: {e}")))?;
    let _ = engine.destroy(vm).await;

    let events = store.stored_events();
    let usage_event = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::KernelUsageRecorded(_)))
        .ok_or_else(|| {
            ConformanceError::events(
                "no KernelUsageRecorded event emitted after successful dispatch",
            )
        })?;

    if let EventKind::KernelUsageRecorded(payload) = &usage_event.kind {
        let expected = default_ctx().wallet;
        if payload.wallet != expected {
            return Err(ConformanceError::events(format!(
                "KernelUsageRecorded.wallet = {:?}, expected {:?}",
                payload.wallet, expected
            )));
        }
        Ok(())
    } else {
        unreachable!("matched KernelUsageRecorded above")
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

async fn discover_backend_name(
    engine: &dyn aios_protocol::ports::KernelPort,
) -> Result<String, ConformanceError> {
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

/// Run every metering scenario in order.
pub async fn run(harness: &dyn ConformanceHarness) -> Result<(), ConformanceError> {
    dispatch_populates_duration_ms(harness).await?;
    dispatch_confidence_is_measured_or_estimated(harness).await?;
    usage_recorded_event_attributes_wallet(harness).await?;
    Ok(())
}
