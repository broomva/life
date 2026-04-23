//! In-process implementation of [`aios_protocol::ports::KernelPort`].
//!
//! The [`engine::KernelEngine`] composes a
//! [`aios_protocol::hypervisor::HypervisorBackend`] with a gate chain
//! (policy → budget → fork-λ → network-isolation) and a
//! [`metering::MeteringWrapper`]. Every state transition is emitted as a
//! `kernel.*` event into the
//! [`aios_protocol::ports::EventStorePort`] provided at construction.
//!
//! ## Crate-level invariants
//!
//! - **Zero hidden state.** [`backend_registry::BackendRegistry`] holds
//!   only the explicit registration map supplied by its caller;
//!   [`event_emitter::EventEmitter`] is immutable post-build;
//!   [`metering::MeteringWrapper`] is a pure wrapper that never
//!   accumulates state across calls. This keeps the engine's observable
//!   behaviour a pure function of the `EventStorePort` journal plus the
//!   current call.
//! - **Determinism.** [`event_emitter::EventEmitter::emit`] with a frozen
//!   clock produces byte-identical
//!   [`aios_protocol::event::EventRecord`]s across runs — the foundation
//!   of the forthcoming replay-determinism test (BRO-876).
//!
//! ## Module layout
//!
//! - [`backend_registry`] — thread-safe map from
//!   [`aios_protocol::hypervisor::BackendSelector`] to
//!   [`aios_protocol::hypervisor::HypervisorBackend`] implementations.
//! - [`event_emitter`] — stamps
//!   [`aios_protocol::event::EventRecord`] envelopes (session, agent,
//!   branch, timestamp) and persists them via the
//!   [`aios_protocol::ports::EventStorePort`] trait object.
//! - [`metering`] — wraps
//!   [`aios_protocol::hypervisor::HypervisorBackend::exec`] with
//!   wall-clock duration accounting and emits
//!   `KernelDispatchStarted` → `KernelDispatchCompleted` →
//!   `KernelUsageRecorded` for each dispatch.
//! - [`dispatch`], [`engine`], [`gate_chain`] — placeholders for the
//!   sibling ticket BRO-870 which wires these three building blocks into
//!   a `KernelPort` implementation.

#![deny(unsafe_code)]

pub mod backend_registry;
pub mod dispatch;
pub mod engine;
pub mod event_emitter;
pub mod gate_chain;
pub mod metering;

pub use backend_registry::{BackendRegistry, RegistryError};
pub use engine::{
    KernelEngine, KernelEngineBuilder, KernelEngineError, ReplayedSnapshot, ReplayedState,
    ReplayedVm,
};
pub use event_emitter::{EventEmitter, EventEmitterBuilder};
pub use gate_chain::{GateChain, GateChainBuildError, GateChainBuilder, GateDecision};
pub use metering::MeteringWrapper;
