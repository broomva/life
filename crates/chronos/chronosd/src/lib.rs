//! chronosd — daemon library.
//!
//! The thin `main.rs` parses CLI args and calls into [`run`]. Splitting the runtime into a
//! library function mirrors the haimad / autonomicd convention and lets the test suite exercise
//! the daemon's setup/shutdown path without spawning a subprocess.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chronos_core::{WakeRouter, WakeTrigger};
use chronos_lago::{CHRONOS_DEFAULT_BRANCH, CHRONOS_SYSTEM_SESSION, record_wake};
use chronos_triggers::HeartbeatTrigger;
use lago_core::id::{BranchId, SessionId};
use lago_core::journal::Journal;
use lago_journal::RedbJournal;
use tokio::sync::oneshot;
use tracing::{info, warn};

/// All configuration the daemon needs to start. Built from CLI / config-file in `main.rs`.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Heartbeat tick interval. Default 10s (dev); 60s recommended for production.
    pub heartbeat_interval: Duration,
    /// Directory containing the lago redb journal. Will be created if it doesn't exist.
    pub data_dir: PathBuf,
    /// Optional explicit lago file name within `data_dir`. Defaults to `journal.redb`.
    pub journal_filename: String,
    /// Internal mpsc capacity for the [`WakeRouter`].
    pub router_buffer: usize,
}

impl DaemonConfig {
    /// Resolve the journal redb file path inside the data directory.
    pub fn journal_path(&self) -> PathBuf {
        self.data_dir.join(&self.journal_filename)
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(10),
            data_dir: PathBuf::from("/tmp/chronosd"),
            journal_filename: "journal.redb".to_string(),
            router_buffer: 64,
        }
    }
}

/// Run the daemon. Returns when the shutdown signal fires AND the router drains.
///
/// `shutdown` is an optional oneshot receiver. When `Some`, the caller can trigger a clean
/// shutdown programmatically (used by tests). When `None`, the daemon installs SIGTERM/SIGINT
/// handlers and shuts down on those instead.
pub async fn run(config: DaemonConfig, shutdown: Option<oneshot::Receiver<()>>) -> Result<()> {
    info!(
        heartbeat_seconds = config.heartbeat_interval.as_secs(),
        data_dir = %config.data_dir.display(),
        journal = %config.journal_filename,
        "chronosd starting"
    );

    // 1. Materialize data directory + lago journal.
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("creating data dir {}", config.data_dir.display()))?;
    let journal_path = config.journal_path();
    let journal: Arc<dyn Journal> = Arc::new(
        RedbJournal::open(&journal_path)
            .with_context(|| format!("opening redb journal at {}", journal_path.display()))?,
    );

    // 2. Default session/branch the daemon writes its system wakes into.
    let fallback_session = SessionId::from_string(CHRONOS_SYSTEM_SESSION);
    let branch = BranchId::from_string(CHRONOS_DEFAULT_BRANCH);

    // 3. Build the router. M0 only wires the heartbeat trigger; the stubs are deliberately
    //    NOT added — they'd just return None immediately and confuse the trace.
    let mut router = WakeRouter::new(config.router_buffer);
    let heartbeat: Box<dyn WakeTrigger> =
        Box::new(HeartbeatTrigger::new(config.heartbeat_interval));
    router.add_trigger(heartbeat);

    // 4. Shutdown plumbing.
    let mut shutdown_rx = shutdown;

    // 5. Main loop.
    let mut wake_count = 0_u64;
    loop {
        tokio::select! {
            event = router.next_wake() => {
                let Some(event) = event else {
                    warn!("router exhausted; exiting main loop");
                    break;
                };
                match record_wake(journal.clone(), &event, &fallback_session, &branch).await {
                    Ok(seq) => {
                        wake_count += 1;
                        info!(
                            seq,
                            wake_total = wake_count,
                            source = event.source.as_str(),
                            "wake recorded"
                        );
                    }
                    Err(err) => {
                        warn!(error = %err, "failed to record wake; continuing");
                    }
                }
            }
            _ = wait_for_shutdown(&mut shutdown_rx) => {
                info!(wake_total = wake_count, "shutdown signal received");
                break;
            }
        }
    }

    // 6. Drain the router. Bounded — shutdown should complete in well under 2 seconds.
    router.shutdown().await;
    info!(wake_total = wake_count, "chronosd shut down cleanly");
    Ok(())
}

/// Wait for either the supplied programmatic shutdown receiver to fire OR an OS signal
/// (SIGTERM / SIGINT on Unix; Ctrl-C on Windows). Returns when any of them resolves.
async fn wait_for_shutdown(programmatic: &mut Option<oneshot::Receiver<()>>) {
    // If a programmatic shutdown channel is configured, use it; otherwise fall through to
    // the OS signal handlers.
    if let Some(rx) = programmatic.as_mut() {
        let _ = rx.await;
        return;
    }

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGTERM handler; falling back to ctrl-c");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGINT handler; falling back to ctrl-c");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = sigterm.recv() => info!("SIGTERM received"),
            _ = sigint.recv() => info!("SIGINT received"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        info!("ctrl-c received");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lago_core::id::{BranchId, SessionId};
    use lago_core::journal::{EventQuery, Journal};
    use tokio::sync::oneshot;
    use tokio::time::sleep;

    use super::*;

    #[tokio::test]
    async fn daemon_starts_writes_a_heartbeat_and_shuts_down_on_signal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = DaemonConfig {
            heartbeat_interval: Duration::from_millis(40),
            data_dir: tmp.path().to_path_buf(),
            journal_filename: "journal.redb".into(),
            router_buffer: 16,
        };

        let (tx, rx) = oneshot::channel();
        let cfg_for_task = cfg.clone();
        let daemon = tokio::spawn(async move { run(cfg_for_task, Some(rx)).await });

        // Let the daemon emit a handful of heartbeats.
        sleep(Duration::from_millis(200)).await;
        tx.send(()).expect("signal shutdown");

        let started_at = std::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(2), daemon)
            .await
            .expect("daemon shutdown within 2s")
            .expect("daemon task ok")
            .expect("daemon run ok");
        let elapsed = started_at.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "shutdown took too long: {elapsed:?}"
        );

        // Read the journal back and confirm at least one chronos.wake landed.
        let journal: Arc<dyn Journal> =
            Arc::new(RedbJournal::open(cfg.journal_path()).expect("reopen redb"))
                as Arc<dyn Journal>;
        let query = EventQuery::new()
            .session(SessionId::from_string(CHRONOS_SYSTEM_SESSION))
            .branch(BranchId::from_string(CHRONOS_DEFAULT_BRANCH));
        let events = journal.read(query).await.expect("read");
        assert!(
            !events.is_empty(),
            "at least one heartbeat wake should be persisted"
        );
        for envelope in &events {
            match &envelope.payload {
                aios_protocol::EventKind::Custom { event_type, data } => {
                    assert_eq!(event_type, chronos_lago::CHRONOS_WAKE_EVENT_TYPE);
                    assert_eq!(data["source"], "heartbeat");
                }
                other => panic!("expected EventKind::Custom, got {other:?}"),
            }
        }
    }
}
