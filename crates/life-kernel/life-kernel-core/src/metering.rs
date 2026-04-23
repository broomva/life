//! Pure wrapper around
//! [`aios_protocol::hypervisor::HypervisorBackend::exec`] that emits the
//! canonical `KernelDispatchStarted` → `KernelDispatchCompleted` →
//! `KernelUsageRecorded` sequence with per-dispatch
//! [`aios_protocol::budget::ResourceUsage`].
//!
//! Fleshed out in BRO-869 Commit 4.
