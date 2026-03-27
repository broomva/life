//! Session adapters — bridge between the relay daemon and local agent runtimes.

pub mod claude;

use life_relay_core::{RelayResult, SessionInfo, SpawnConfig};
use tokio::sync::mpsc;

/// Trait for session adapters (Claude Code, Codex, Arcan).
#[async_trait::async_trait]
pub trait SessionAdapter: Send + Sync {
    /// Spawn a new session. Returns session info.
    async fn spawn(&self, config: &SpawnConfig) -> RelayResult<SessionInfo>;

    /// Send input (text or keystrokes) to the session.
    async fn send_input(&self, session_id: &uuid::Uuid, data: &str) -> RelayResult<()>;

    /// Kill the session.
    async fn kill(&self, session_id: &uuid::Uuid) -> RelayResult<()>;

    /// Start streaming output. Sends output strings to the channel.
    async fn stream_output(
        &self,
        session_id: &uuid::Uuid,
        tx: mpsc::Sender<String>,
    ) -> RelayResult<()>;
}
