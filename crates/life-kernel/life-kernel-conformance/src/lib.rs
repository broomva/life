//! Backend-agnostic conformance test battery for
//! [`aios_protocol::ports::KernelPort`] implementations.
//!
//! Backends plug in via the [`ConformanceHarness`] trait: a harness
//! returns a fully-wired [`KernelEngine`] plus the in-memory
//! [`CapturingEventStore`] it was built against, so each scenario can
//! drive the engine and inspect the resulting `kernel.*` event trail.
//!
//! # Scope
//!
//! Phase 1 ships four batteries:
//!
//! - [`lifecycle`] — create / dispatch / snapshot / fork / destroy round-trips.
//! - [`errors`] — negative paths: dispatch after destroy, missing
//!   snapshots, capability gaps, timeouts.
//! - [`metering`] — [`aios_protocol::budget::ResourceUsage`] is
//!   populated, confidence is reported, and
//!   `KernelUsageRecorded` carries the expected wallet.
//! - [`events`] — kernel event order + causation + gate denial trail.
//!
//! Run them individually via the `run(&harness)` fn each module
//! exposes, or in one shot via [`run_all_conformance_tests`].
//!
//! # Capability-gated scenarios
//!
//! Some scenarios require backend capabilities that not every backend
//! advertises (e.g. [`BackendCapabilitySet::PERSISTENCE`] for
//! snapshot/fork). Those scenarios detect the missing capability and
//! return `Ok(())` after a single `eprintln!` note; they never fail a
//! backend for a gap it legitimately does not cover.
//!
//! [`KernelEngine`]: life_kernel_core::KernelEngine
//! [`BackendCapabilitySet`]: aios_protocol::hypervisor::BackendCapabilitySet

#![deny(unsafe_code)]

use std::sync::Arc;

use aios_protocol::event::EventRecord;
use aios_protocol::ports::EventStorePort;
use async_trait::async_trait;
use life_kernel_core::KernelEngine;

pub mod lifecycle;

/// Extension on [`EventStorePort`] that lets scenarios read back the
/// events the engine wrote.
///
/// Implementations MUST return every event ever appended, in append
/// order, cloned so the caller can inspect sequence / kind / causation
/// without racing the engine.
pub trait CapturingEventStore: EventStorePort {
    /// Every event recorded so far, in append order.
    fn stored_events(&self) -> Vec<EventRecord>;
}

/// Harness a backend integrator implements so their
/// [`KernelEngine`](life_kernel_core::KernelEngine) can be driven by the
/// conformance scenarios.
///
/// The harness owns the event store so scenarios can inspect the event
/// trail without going through the `KernelPort` trait. Implementations
/// are expected to return a *fresh* engine + store on every call —
/// scenarios assume an empty journal at construction time.
#[async_trait]
pub trait ConformanceHarness: Send + Sync {
    /// Build a fresh engine + capturing store pair.
    async fn build_engine(&self) -> (KernelEngine, Arc<dyn CapturingEventStore>);

    /// Optional variant that wires an always-deny policy gate.
    ///
    /// Used by [`events::gate_deny_emits_dispatch_denied`]. Defaults to
    /// [`build_engine`](Self::build_engine) — scenarios that require a
    /// deny-policy wiring will skip gracefully when the default is
    /// returned (i.e. when the harness has not overridden this method).
    async fn build_engine_with_deny_policy(
        &self,
    ) -> Option<(KernelEngine, Arc<dyn CapturingEventStore>)> {
        None
    }
}

/// Error surface returned by conformance scenarios.
///
/// A scenario fails loudly on behaviour divergence; skipping a
/// capability-gated scenario returns `Ok(())` (see the crate-level
/// rustdoc) rather than a variant here.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConformanceError {
    /// The engine returned a value that violates the
    /// [`aios_protocol::ports::KernelPort`] contract.
    #[error("contract violation: {0}")]
    Contract(String),
    /// A call that the contract requires to succeed surfaced an error.
    #[error("expected success but got error: {0}")]
    UnexpectedError(String),
    /// A call that the contract requires to fail returned a success
    /// value.
    #[error("expected error but got success: {0}")]
    ExpectedFailure(String),
    /// The emitted event stream did not match the contract.
    #[error("event trail mismatch: {0}")]
    EventMismatch(String),
    /// A required assertion on usage / metering data failed.
    #[error("metering mismatch: {0}")]
    MeteringMismatch(String),
}

impl ConformanceError {
    /// Construct a [`ConformanceError::Contract`] with a formatted
    /// message.
    pub fn contract(msg: impl Into<String>) -> Self {
        Self::Contract(msg.into())
    }

    /// Construct a [`ConformanceError::EventMismatch`] with a formatted
    /// message.
    pub fn events(msg: impl Into<String>) -> Self {
        Self::EventMismatch(msg.into())
    }

    /// Construct a [`ConformanceError::MeteringMismatch`] with a
    /// formatted message.
    pub fn metering(msg: impl Into<String>) -> Self {
        Self::MeteringMismatch(msg.into())
    }
}

/// Run every conformance battery against `harness`.
///
/// The function short-circuits at the first failure — the returned
/// error names the battery and scenario that tripped.
///
/// Later commits in this task stack add the `errors`, `metering`, and
/// `events` modules; this top-level runner is updated in lockstep.
pub async fn run_all_conformance_tests(
    harness: &dyn ConformanceHarness,
) -> Result<(), ConformanceError> {
    lifecycle::run(harness).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conformance_error_constructors_work() {
        let e = ConformanceError::contract("foo");
        assert!(matches!(e, ConformanceError::Contract(_)));
        let e = ConformanceError::events("bar");
        assert!(matches!(e, ConformanceError::EventMismatch(_)));
        let e = ConformanceError::metering("baz");
        assert!(matches!(e, ConformanceError::MeteringMismatch(_)));
    }
}
