//! Session adapters — bridge between the relay daemon and local agent runtimes.

pub mod claude;
pub mod parser;
pub mod pty;

use life_relay_core::{DaemonMessage, RelayResult, SessionInfo, SpawnConfig};
use uuid::Uuid;

/// Trait for session adapters (Claude Code, Codex, Arcan).
///
/// Implementations manage a collection of live sessions internally.
/// Output is streamed to the provided `event_tx` channel during `spawn`.
#[async_trait::async_trait]
pub trait SessionAdapter: Send + Sync {
    /// Spawn a new session. Returns session info.
    ///
    /// Starts a background task that sends [`DaemonMessage`] events
    /// (output, approvals, ended) to `event_tx` for the lifetime of the session.
    async fn spawn(
        &self,
        config: &SpawnConfig,
        event_tx: tokio::sync::mpsc::Sender<DaemonMessage>,
    ) -> RelayResult<SessionInfo>;

    /// Send input (text or keystrokes) to the session.
    async fn send_input(&self, session_id: &Uuid, data: &str) -> RelayResult<()>;

    /// Kill the session.
    async fn kill(&self, session_id: &Uuid) -> RelayResult<()>;

    /// Resize the session terminal.
    async fn resize(&self, session_id: &Uuid, cols: u16, rows: u16) -> RelayResult<()>;
}
