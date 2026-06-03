//! Chronos kernel-wake-handoff wiring for `arcan serve --chronos` (M2, BRO-1080).
//!
//! This is the *only* place chronos types meet arcand/aios types. [`ArcandKernelDispatcher`]
//! implements `chronos_core::KernelDispatcher` by ensuring the target session exists and calling
//! `KernelRuntime::tick_on_branch` — an actual agent tick. [`spawn_chronos`] builds the
//! [`WakeRouter`] (heartbeat + HTTP trigger), the lago-backed agenda, the `chronos-api` wake-ingest
//! server, and the `run_kernel_wake_loop` background task — all opt-in, so a plain `arcan serve`
//! (no `--chronos`) is byte-for-byte unchanged.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aios_protocol::{BranchId, ModelRouting, PolicySet};
use aios_runtime::{KernelRuntime, TickInput, TickKind};
use async_trait::async_trait;
use chronos_api::ApiState;
use chronos_core::{
    AgendaStore, ChronosError, ChronosResult, DispatchOutcome, KernelDispatcher,
    SessionId as ChronosSessionId, WakeRouter, WakeTrigger,
};
use chronos_lago::{
    CHRONOS_DEFAULT_BRANCH, CHRONOS_SYSTEM_SESSION, LagoAgendaStore, run_kernel_wake_loop,
};
use chronos_triggers::{HeartbeatTrigger, wake_channel};
use lago_core::id::{BranchId as LagoBranchId, SessionId as LagoSessionId};
use lago_core::journal::Journal;

/// `chronos_core::KernelDispatcher` over the live `KernelRuntime` — the M2 handoff into the
/// canonical tick engine. Chronos wakes are system actors, so this bypasses the HTTP-auth machinery
/// in `run_session` and calls `tick_on_branch` directly (the M2 constraint: "call tick_on_branch
/// from outside the loop body").
pub struct ArcandKernelDispatcher {
    runtime: Arc<KernelRuntime>,
}

impl ArcandKernelDispatcher {
    /// Wrap a shared kernel runtime handle.
    pub fn new(runtime: Arc<KernelRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl KernelDispatcher for ArcandKernelDispatcher {
    async fn dispatch(
        &self,
        session_id: &ChronosSessionId,
        intent: &str,
    ) -> ChronosResult<DispatchOutcome> {
        // chronos_core::SessionId IS aios_protocol::SessionId (re-export), so the runtime accepts
        // it directly. Chronos always dispatches on the `main` branch (the M0 convention).
        let branch = BranchId::main();

        if !self.runtime.session_exists(session_id) {
            self.runtime
                .create_session_with_id(
                    session_id.clone(),
                    "chronos",
                    PolicySet::default(),
                    ModelRouting::default(),
                )
                .await
                .map_err(|e| {
                    ChronosError::Trigger(format!("chronos: create session failed: {e}"))
                })?;
        }

        let input = TickInput {
            objective: intent.to_string(),
            proposed_tool: None,
            system_prompt: None,
            allowed_tools: None,
            kind: TickKind::Direct,
        };

        // A tick error is a dispatch *failure* (recorded as chronos.agenda.failed by the wake
        // loop), not a loop-fatal error — so it folds into the outcome rather than propagating.
        match self
            .runtime
            .tick_on_branch(session_id, &branch, input)
            .await
        {
            Ok(_output) => Ok(DispatchOutcome::completed(0)),
            Err(err) => Ok(DispatchOutcome::failed(err.to_string())),
        }
    }
}

/// Wire the Chronos wake-loop into the running runtime (M2, opt-in via `--chronos`).
///
/// Spawns (1) the `chronos-api` HTTP wake-ingest server when `http_bind` is set, and (2) the
/// `run_kernel_wake_loop` background task that drives each wake into [`ArcandKernelDispatcher`] and
/// records the agenda outcome. Must be called from within the tokio runtime context (arcand enters
/// it via `_rt_guard` before this runs, so the spawned tasks queue and execute once the server's
/// `block_on` starts).
pub fn spawn_chronos(
    runtime: Arc<KernelRuntime>,
    journal: Arc<dyn Journal>,
    http_bind: Option<SocketAddr>,
    heartbeat_secs: u64,
) {
    // Router: heartbeat pulse (journaled, never dispatched — no intent) + the HTTP wake source.
    let mut router = WakeRouter::new(64);
    router.add_trigger(Box::new(HeartbeatTrigger::new(Duration::from_secs(
        heartbeat_secs.max(60),
    ))) as Box<dyn WakeTrigger>);
    let (wake_tx, http_trigger) = wake_channel(64);
    router.add_trigger(Box::new(http_trigger));

    let agenda: Arc<LagoAgendaStore> = Arc::new(LagoAgendaStore::new(journal.clone()));
    let dispatcher = Arc::new(ArcandKernelDispatcher::new(runtime));

    // HTTP wake-ingest API (POST /v1/wake, GET /v1/agenda/{session}) when bound.
    if let Some(addr) = http_bind {
        let state = ApiState {
            agenda: agenda.clone(),
            wake_tx,
            default_session: ChronosSessionId::from_string(CHRONOS_SYSTEM_SESSION),
        };
        tokio::spawn(async move {
            // Runs for the lifetime of the daemon; aborted when the runtime drops on shutdown.
            if let Err(err) = chronos_api::serve(addr, state, std::future::pending::<()>()).await {
                tracing::warn!(error = %err, "chronos-api server exited with error");
            }
        });
        tracing::info!(%addr, "chronos wake-ingest API listening (M2 --chronos)");
    } else {
        drop(wake_tx); // heartbeat-only; no HTTP source.
        tracing::info!("chronos enabled, heartbeat-only (no --chronos-http-bind)");
    }

    // The kernel wake loop: next_wake -> record_wake -> dispatch -> agenda transition.
    let default_session = LagoSessionId::from_string(CHRONOS_SYSTEM_SESSION);
    let branch = LagoBranchId::from_string(CHRONOS_DEFAULT_BRANCH);
    tokio::spawn(async move {
        run_kernel_wake_loop(
            &mut router,
            journal,
            agenda.as_ref() as &dyn AgendaStore,
            dispatcher.as_ref() as &dyn KernelDispatcher,
            &default_session,
            &branch,
        )
        .await;
    });
    tracing::info!("chronos kernel wake-loop started (M2 --chronos)");
}
