//! Adapter between lifed-originated agent task invocations and arcan's
//! ConsciousnessActor runtime.
//!
//! Per Lumen spec L6-D5 + L6-D10:
//! - lifed.Agent.CreateSession invocations (from lifegw's warp_graphql
//!   createAgentTask resolver) reach arcan via this trait. A concrete
//!   `WarpTaskBackend` impl (lives in arcand or a glue crate) constructs
//!   a `ConsciousnessActor` scoped to (user_id, task_id) and runs it.
//! - The actor emits AmbientTaskUpdated frames via the existing arcan
//!   event bus → lifed.EventService → lifegw warpDriveUpdates
//!   subscription → Warp client.
//! - L6-D10 frontier-vision constraint: this entry point is callable
//!   both from lifed (reactive, today) and from arcan itself (proactive,
//!   future). The signature reflects that — `WarpTaskSpawnInput` is
//!   source-agnostic.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Source-agnostic input sufficient to spawn a ConsciousnessActor for a
/// Warp-originated task. Serves both reactive (Warp asks via createAgentTask)
/// and proactive (Life decides) invocations.
#[derive(Debug, Clone)]
pub struct WarpTaskSpawnInput {
    /// Owning user. Scopes all downstream events + custody to a single
    /// principal.
    pub user_id: String,
    /// Stable task identifier — matches the `task_id` surfaced on
    /// `warpDriveUpdates`.
    pub task_id: String,
    /// Initial prompt provided by the caller.
    pub prompt: String,
    /// Model the actor should default to. Resolution against provider
    /// catalogs is the backend's responsibility.
    pub model_id: String,
    /// Workspace scope (filesystem root + skill set).
    pub workspace_id: String,
    /// Optional parent task — set when this is a spawned sub-task.
    pub parent_task_id: Option<String>,
    /// Trace identifier propagated for OTel correlation.
    pub trace_id: String,
}

/// Control message sent to a running task via `WarpTaskBackend::control`.
/// Maps to upstream Warp's `updateAgentTask` mutation actions.
#[derive(Debug, Clone)]
pub enum WarpTaskAction {
    /// Request graceful cancellation. The backend surfaces a typed close
    /// code on the `warpDriveUpdates` subscription per Spec C₃ §6.5.
    Cancel,
    /// Approve or reject a pending tool call by id.
    ApproveToolCall {
        /// Identifier of the tool call awaiting approval.
        call_id: String,
        /// `true` to approve, `false` to reject.
        approved: bool,
    },
    /// Deliver follow-up user input to a running task.
    SendUserInput {
        /// User-provided text appended to the conversation.
        text: String,
    },
}

/// Backend that owns ConsciousnessActor spawn lifecycle. Implementations
/// live in glue crates (arcand, or a future life-runtime crate) that
/// depend on both arcan-core and lifed. Keeping the trait in arcan-core
/// means arcan-core stays free of lifed/lifegw dependencies.
#[async_trait]
pub trait WarpTaskBackend: Send + Sync {
    /// Spawn a new ConsciousnessActor scoped to (user_id, task_id).
    /// Returns once the actor is alive and accepting events; the actor
    /// itself runs to completion in the background.
    async fn spawn(&self, input: WarpTaskSpawnInput) -> Result<()>;

    /// Send a control message to a running task. Per Lumen spec L6-D5
    /// + Spec C₃ §6.5, cancellation surfaces a typed close-code on the
    /// `warpDriveUpdates` subscription stream so the client can react
    /// appropriately.
    async fn control(&self, user_id: &str, task_id: &str, action: WarpTaskAction) -> Result<()>;
}

/// Thin wrapper holding an `Arc<dyn WarpTaskBackend>`. Callers (the
/// lifegw->lifed->arcan bridge) inject a backend at startup and invoke
/// `spawn` / `control` per request.
#[derive(Clone)]
pub struct WarpTaskAdapter {
    inner: Arc<dyn WarpTaskBackend>,
}

impl WarpTaskAdapter {
    /// Construct an adapter wrapping the given backend.
    pub fn new(backend: Arc<dyn WarpTaskBackend>) -> Self {
        Self { inner: backend }
    }

    /// Forward to `WarpTaskBackend::spawn`.
    pub async fn spawn(&self, input: WarpTaskSpawnInput) -> Result<()> {
        self.inner.spawn(input).await
    }

    /// Forward to `WarpTaskBackend::control`.
    pub async fn control(
        &self,
        user_id: &str,
        task_id: &str,
        action: WarpTaskAction,
    ) -> Result<()> {
        self.inner.control(user_id, task_id, action).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test backend that records calls without actually spawning actors.
    /// Lets us verify the adapter's wiring contract without depending on
    /// the real ConsciousnessActor.
    #[derive(Default)]
    struct FakeBackend {
        spawned: Mutex<Vec<WarpTaskSpawnInput>>,
        controls: Mutex<Vec<(String, String, WarpTaskAction)>>,
    }

    #[async_trait]
    impl WarpTaskBackend for FakeBackend {
        async fn spawn(&self, input: WarpTaskSpawnInput) -> Result<()> {
            self.spawned.lock().unwrap().push(input);
            Ok(())
        }

        async fn control(
            &self,
            user_id: &str,
            task_id: &str,
            action: WarpTaskAction,
        ) -> Result<()> {
            self.controls
                .lock()
                .unwrap()
                .push((user_id.to_string(), task_id.to_string(), action));
            Ok(())
        }
    }

    #[tokio::test]
    async fn spawn_records_input() {
        let fake = Arc::new(FakeBackend::default());
        let adapter = WarpTaskAdapter::new(fake.clone());
        adapter
            .spawn(WarpTaskSpawnInput {
                user_id: "u1".into(),
                task_id: "t1".into(),
                prompt: "hi".into(),
                model_id: "claude-sonnet-4".into(),
                workspace_id: "ws".into(),
                parent_task_id: None,
                trace_id: "tr".into(),
            })
            .await
            .unwrap();
        assert_eq!(fake.spawned.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn control_cancel_records_action() {
        let fake = Arc::new(FakeBackend::default());
        let adapter = WarpTaskAdapter::new(fake.clone());
        adapter
            .control("u1", "t1", WarpTaskAction::Cancel)
            .await
            .unwrap();
        assert_eq!(fake.controls.lock().unwrap().len(), 1);
        assert!(matches!(
            fake.controls.lock().unwrap()[0].2,
            WarpTaskAction::Cancel
        ));
    }
}
