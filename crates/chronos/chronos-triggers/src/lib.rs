//! Wake-trigger implementations for Chronos.
//!
//! M0 ships [`HeartbeatTrigger`] (a periodic timer) and stub placeholders for the other
//! source types. Stubs implement [`chronos_core::WakeTrigger`] but return `None` from
//! `next_wake` — they're present so the [`chronos_core::WakeRouter`] can be wired with
//! every taxonomy variant from day one, without breaking the trait once real impls land.
//!
//! ## Per-milestone roadmap
//!
//! - **M0** (this crate, today): [`HeartbeatTrigger`], stubs for the rest.
//! - **M1**: real [`HttpTrigger`] backed by an axum server in `chronos-api`.
//! - **M3**: real [`FsWatchTrigger`] (via the `notify` crate) and `SubAgentReturnTrigger`.
//! - **Beyond**: cron, webhook, threshold.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod heartbeat;
mod stubs;

pub use heartbeat::HeartbeatTrigger;
pub use stubs::{
    CronTriggerStub, FsWatchTriggerStub, HttpTriggerStub, SubAgentReturnTriggerStub,
    ThresholdTriggerStub, WebhookTriggerStub,
};
