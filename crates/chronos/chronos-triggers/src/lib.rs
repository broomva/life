//! Wake-trigger implementations for Chronos.
//!
//! M0 ships [`HeartbeatTrigger`] (a periodic timer) and stub placeholders for the other
//! source types. Stubs implement [`chronos_core::WakeTrigger`] but return `None` from
//! `next_wake` — they're present so the [`chronos_core::WakeRouter`] can be wired with
//! every taxonomy variant from day one, without breaking the trait once real impls land.
//!
//! ## Per-milestone roadmap
//!
//! - **M0**: [`HeartbeatTrigger`], stubs for the rest.
//! - **M1** (today): real [`HttpTrigger`] fed by the `chronos-api` axum server via [`wake_channel`].
//! - **M3**: real [`FsWatchTrigger`] (via the `notify` crate) and `SubAgentReturnTrigger`.
//! - **Beyond**: cron, webhook, threshold.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod heartbeat;
mod http;
mod stubs;

pub use heartbeat::HeartbeatTrigger;
pub use http::{HttpTrigger, WakeSender, wake_channel};
pub use stubs::{
    CronTriggerStub, FsWatchTriggerStub, SubAgentReturnTriggerStub, ThresholdTriggerStub,
    WebhookTriggerStub,
};
