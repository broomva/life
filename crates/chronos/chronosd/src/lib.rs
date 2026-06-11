//! chronosd — daemon library.
//!
//! The thin `main.rs` parses CLI args and calls into [`run`]. Splitting the runtime into a
//! library function mirrors the haimad / autonomicd convention and lets the test suite exercise
//! the daemon's setup/shutdown path without spawning a subprocess.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chronos_api::ApiState;
use chronos_core::{AgendaStore, WakeRouter, WakeTrigger};
use chronos_lago::{CHRONOS_DEFAULT_BRANCH, CHRONOS_SYSTEM_SESSION, LagoAgendaStore, record_wake};
use chronos_triggers::{HeartbeatTrigger, wake_channel};
use lago_core::id::{BranchId, SessionId};
use lago_core::journal::Journal;
use lago_journal::RedbJournal;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
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
    /// Optional bind address for the M1 HTTP wake-ingest API (`chronos-api`). When `None`,
    /// chronosd runs heartbeat-only — exactly the M0 behavior.
    pub http_bind: Option<SocketAddr>,
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
            http_bind: None,
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

    // 3. Build the router. The heartbeat trigger is always wired; the M0 stubs are deliberately
    //    NOT added — they'd just return None immediately and confuse the trace.
    let mut router = WakeRouter::new(config.router_buffer);
    let heartbeat: Box<dyn WakeTrigger> =
        Box::new(HeartbeatTrigger::new(config.heartbeat_interval));
    router.add_trigger(heartbeat);

    // 3b. M1 — HTTP wake ingest. When `--http-bind` is set, register a real HttpTrigger fed by
    //     the chronos-api server, backed by the lago-projection agenda store. The trigger drains
    //     into the same router/record_wake path as the heartbeat — wakes from either source are
    //     journaled identically.
    let (api_shutdown_tx, api_shutdown_rx) = oneshot::channel::<()>();
    let mut api_handle: Option<JoinHandle<()>> = None;
    if let Some(addr) = config.http_bind {
        let (wake_tx, http_trigger) = wake_channel(config.router_buffer);
        router.add_trigger(Box::new(http_trigger));

        let agenda: Arc<dyn AgendaStore> = Arc::new(LagoAgendaStore::new(journal.clone()));
        let api_state = ApiState {
            agenda,
            wake_tx,
            // chronos-api stays free of routing constants; inject the system session here.
            default_session: chronos_core::SessionId::from_string(CHRONOS_SYSTEM_SESSION),
        };
        info!(%addr, "chronos-api enabled — M1 wake ingest (POST /v1/wake, GET /v1/agenda/{{id}})");
        api_handle = Some(tokio::spawn(async move {
            let shutdown = async move {
                let _ = api_shutdown_rx.await;
            };
            if let Err(err) = chronos_api::serve(addr, api_state, shutdown).await {
                warn!(error = %err, "chronos-api server exited with error");
            }
        }));
    } else {
        // No API: drop the receiver so the unused shutdown sender is harmless.
        drop(api_shutdown_rx);
    }

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

    // 6. Stop the HTTP server (if running) and wait for it to drain. Dropping its ApiState drops
    //    the wake sender, so the HttpTrigger exhausts cleanly when we drain the router next.
    let _ = api_shutdown_tx.send(());
    if let Some(handle) = api_handle {
        let _ = handle.await;
    }

    // 7. Drain the router. Bounded — shutdown should complete in well under 2 seconds.
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
            heartbeat_interval: Duration::from_millis(20),
            data_dir: tmp.path().to_path_buf(),
            journal_filename: "journal.redb".into(),
            router_buffer: 16,
            http_bind: None,
        };

        let (tx, rx) = oneshot::channel();
        let cfg_for_task = cfg.clone();
        let daemon = tokio::spawn(async move { run(cfg_for_task, Some(rx)).await });

        // Let the daemon emit a handful of heartbeats. The window is 50x the
        // interval: loaded CI runners (macOS especially) can stall the spawned
        // task long enough that a tight window records zero beats.
        sleep(Duration::from_millis(1000)).await;
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

    /// End-to-end M1 acceptance: a real `POST /v1/wake` over the wire makes the daemon journal
    /// BOTH a `chronos.agenda.added` event (in the agenda ledger) AND a `chronos.wake` event
    /// (in the target session). Exercises the full API → HttpTrigger → router → record_wake wiring.
    #[tokio::test]
    async fn daemon_http_ingest_records_wake_and_agenda_events() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Grab a free port, then drop the probe so chronosd can bind it.
        let addr: std::net::SocketAddr = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
            probe.local_addr().expect("local addr")
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = DaemonConfig {
            // Long heartbeat so it never fires during the test — isolates the HTTP wake.
            heartbeat_interval: Duration::from_secs(3_600),
            data_dir: tmp.path().to_path_buf(),
            journal_filename: "journal.redb".into(),
            router_buffer: 16,
            http_bind: Some(addr),
        };

        let (tx, rx) = oneshot::channel();
        let cfg_for_task = cfg.clone();
        let daemon = tokio::spawn(async move { run(cfg_for_task, Some(rx)).await });

        let body = r#"{"intent":"smoke via http","session_id":"smoke-sess"}"#;
        let request = format!(
            "POST /v1/wake HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );

        // Retry the connect until the API has bound (≤ ~1s).
        let mut response = String::new();
        let mut connected = false;
        for _ in 0..50 {
            match tokio::net::TcpStream::connect(addr).await {
                Ok(mut stream) => {
                    stream.write_all(request.as_bytes()).await.expect("write");
                    stream.read_to_string(&mut response).await.expect("read");
                    connected = true;
                    break;
                }
                Err(_) => sleep(Duration::from_millis(20)).await,
            }
        }
        assert!(connected, "could not connect to chronos-api");
        let status_line = response.lines().next().unwrap_or_default();
        assert!(
            status_line.starts_with("HTTP/1.1 202"),
            "expected 202 Accepted, got: {status_line}"
        );

        // The agenda.added is journaled synchronously before the 202; the wake flows through the
        // mpsc, so give the router loop a moment to record it.
        sleep(Duration::from_millis(200)).await;
        tx.send(()).expect("signal shutdown");
        tokio::time::timeout(Duration::from_secs(2), daemon)
            .await
            .expect("daemon shutdown within 2s")
            .expect("daemon task ok")
            .expect("daemon run ok");

        let journal: Arc<dyn Journal> =
            Arc::new(RedbJournal::open(cfg.journal_path()).expect("reopen redb"))
                as Arc<dyn Journal>;

        // chronos.agenda.added landed in the dedicated agenda ledger session.
        let agenda = journal
            .read(
                EventQuery::new()
                    .session(SessionId::from_string(chronos_lago::CHRONOS_AGENDA_SESSION))
                    .branch(BranchId::from_string(chronos_lago::CHRONOS_DEFAULT_BRANCH)),
            )
            .await
            .expect("read agenda ledger");
        assert_eq!(agenda.len(), 1, "exactly one agenda.added event");
        match &agenda[0].payload {
            aios_protocol::EventKind::Custom { event_type, data } => {
                assert_eq!(event_type, chronos_lago::CHRONOS_AGENDA_ADDED_EVENT_TYPE);
                assert_eq!(data["intent"], "smoke via http");
                assert_eq!(data["session_id"], "smoke-sess");
            }
            other => panic!("expected EventKind::Custom, got {other:?}"),
        }

        // chronos.wake routed to the target session.
        let wakes = journal
            .read(
                EventQuery::new()
                    .session(SessionId::from_string("smoke-sess"))
                    .branch(BranchId::from_string(chronos_lago::CHRONOS_DEFAULT_BRANCH)),
            )
            .await
            .expect("read target session");
        assert_eq!(wakes.len(), 1, "exactly one chronos.wake event");
        match &wakes[0].payload {
            aios_protocol::EventKind::Custom { event_type, data } => {
                assert_eq!(event_type, chronos_lago::CHRONOS_WAKE_EVENT_TYPE);
                assert_eq!(data["source"], "http");
            }
            other => panic!("expected EventKind::Custom, got {other:?}"),
        }
    }
}
