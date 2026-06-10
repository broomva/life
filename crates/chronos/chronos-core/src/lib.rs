//! Chronos core — types, traits, and the wake router for the temporal substrate of the Life Agent OS.
//!
//! Chronos answers two questions for the kernel:
//!
//! 1. **"When should an agent wake?"** — a unified surface that coalesces all wake sources
//!    (HTTP `/runs`, cron, file changes, sub-agent completions, threshold crossings, webhooks,
//!    heartbeats) into a single ordered event stream the kernel can subscribe to.
//! 2. **"What should the agent do when it wakes?"** — a durable per-session [`AgendaStore`] of
//!    [`AgendaItem`]s that survives daemon restarts (introduced in M1).
//!
//! ## Dependency rule
//!
//! `chronos-core` depends ONLY on `aios-protocol` from the Life internal-crate graph. No lago,
//! no arcan, no autonomic. Enforced by `scripts/architecture/verify_dependencies_chronos.sh`.
//!
//! ## Module shape
//!
//! - [`WakeEvent`] — universal wake shape (id, timestamp, source, payload, optional target session)
//! - [`WakeSource`] — taxonomy of where a wake can come from
//! - [`WakeTrigger`] — async trait every trigger implements (heartbeat, http, cron, fs, …)
//! - [`WakeRouter`] — multiplexes triggers concurrently into a single stream
//! - [`AgendaStore`] / [`AgendaItem`] / [`InMemoryAgendaStore`] — the durable agenda (M1)
//! - [`ChronosError`] / [`ChronosResult`] — crate-local error type
//!
//! ## Why `tokio` in core?
//!
//! [`WakeRouter`] needs an `mpsc` channel to multiplex concurrent triggers. The autonomic-core
//! crate also pulls tokio for the same reasons — workspace convention is "internal Life crates
//! beyond aios-protocol" rule, not "external Rust crates" rule.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::time::{SystemTime, UNIX_EPOCH};

mod agenda;
mod dispatch;
mod error;
mod router;
mod trigger;
mod wake;

pub use agenda::{
    AgendaItem, AgendaItemId, AgendaItemState, AgendaStore, InMemoryAgendaStore, NewAgendaItem,
    Priority, sort_for_dispatch,
};
#[cfg(any(test, feature = "test-util"))]
pub use dispatch::MockKernelDispatcher;
pub use dispatch::{DispatchOutcome, KernelDispatcher, WakeDispatch, wake_dispatch_params};
pub use error::{ChronosError, ChronosResult};
pub use router::WakeRouter;
pub use trigger::WakeTrigger;
pub use wake::{WakeEvent, WakeEventId, WakeSource};

/// Re-export of `aios_protocol::SessionId` so consumers can build [`WakeEvent::target_session`]
/// without taking a direct dependency on aios-protocol themselves.
pub use aios_protocol::SessionId;

/// Return milliseconds since the Unix epoch.
///
/// Used by triggers when constructing [`WakeEvent`] timestamps. Returns 0 if the clock is
/// before the Unix epoch (which would indicate a misconfigured system clock — the wake event
/// is still produced so observability can surface the anomaly).
pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
