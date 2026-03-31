//! Life Relay — core types, traits, and wire protocol.
//!
//! This crate defines the shared contract between the `relayd` daemon
//! (running on the user's machine) and the broomva.tech relay edge.

pub mod error;
pub mod protocol;
pub mod session;

pub use error::{RelayError, RelayResult};
pub use protocol::{DaemonMessage, DirEntry, HistoryMessage, HistoryToolUse, ServerMessage};
pub use session::{SessionInfo, SessionStatus, SessionType, SpawnConfig};
