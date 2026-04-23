//! MVS gate implementations for `life-kernel-core`.
//!
//! Phase 1 ships three permissive-by-default gates that wire the kernel
//! engine end-to-end without enforcing anything substantive:
//!
//! - [`budget::NoOpBudgetGate`] — permits every dispatch + fork; traces the
//!   [`aios_protocol::budget::ResourceBudget`] cost hint for observability.
//! - [`network::NoOpNetworkIsolation`] — `apply` is a no-op; `record_egress`
//!   accumulates into an atomic counter for conformance-suite assertions.
//! - [`policy::StaticPolicyGate`] — wraps an existing
//!   [`aios_protocol::ports::PolicyGatePort`] and maps
//!   [`aios_protocol::ports::PolicyGateDecision`] to the kernel-tier
//!   [`aios_protocol::budget::BudgetDecision`].
//!
//! Real enforcement (session budget, eBPF isolation, RCS-λ fork gate) is
//! Phase 4 / Phase 6 work.

#![deny(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "gate-budget-noop")]
pub mod budget;

#[cfg(feature = "gate-net-noop")]
pub mod network;

#[cfg(feature = "gate-policy-static")]
pub mod policy;

#[cfg(feature = "gate-budget-noop")]
pub use budget::NoOpBudgetGate;

#[cfg(feature = "gate-net-noop")]
pub use network::NoOpNetworkIsolation;

#[cfg(feature = "gate-policy-static")]
pub use policy::StaticPolicyGate;
