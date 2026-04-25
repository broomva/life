//! Transport multiplexer.
//!
//! Phase 2 ships two listeners:
//! - The Unix-socket listener (always active).
//! - The vsock listener (Linux + `vsock-listener` feature; activated when
//!   `cfg.server.vsock` is `Some`).
//!
//! Both share a single tonic `Router` (which is `Clone` in tonic 0.14) and
//! receive their own `oneshot::Receiver<()>` shutdown signal fanned out from a
//! shared `tokio::sync::Notify`.

pub mod unix;

#[cfg(all(target_os = "linux", feature = "vsock-listener"))]
pub mod vsock;

use std::sync::Arc;
use std::time::Duration;

use aios_protocol::hypervisor::VmHandle;
use aios_protocol::ports::KernelPort;
use tokio::sync::oneshot;
use tonic::transport::Server;

use crate::config::SomaConfig;
use crate::error::SomaResult;
use crate::server::LifeKernelService;

/// Spawns every configured listener and awaits shutdown + in-flight drain.
///
/// `seed` is passed to [`LifeKernelService::with_seed`] to populate the
/// live-VM index from the replay state recorded at bootstrap.  Pass
/// `Vec::new()` for tests that do not need seeding.
///
/// Returns `Ok(())` on graceful shutdown. Any listener error bubbles up as
/// `SomaError::Server(..)`.
pub async fn serve<E: KernelPort + 'static>(
    cfg: &SomaConfig,
    engine: Arc<E>,
    shutdown_rx: oneshot::Receiver<()>,
    seed: Vec<VmHandle>,
) -> SomaResult<()> {
    let service = LifeKernelService::with_seed(engine, seed);
    let in_flight = service.in_flight();

    // Router is Clone in tonic 0.14, so we can hand separate copies to each
    // listener when running dual-transport mode.
    let router = Server::builder().add_service(service.into_server());

    // On Linux + vsock-listener feature + vsock config present: run both
    // listeners concurrently, sharing a Notify-based shutdown fan-out.
    #[cfg(all(target_os = "linux", feature = "vsock-listener"))]
    if let Some(vsock_cfg) = cfg.server.vsock.as_ref() {
        serve_unix_and_vsock(cfg, vsock_cfg, router, shutdown_rx).await?;
        drain_after_shutdown(in_flight, &cfg.server).await;
        return Ok(());
    }

    // Default path: unix only.
    unix::serve(&cfg.server, router, shutdown_rx).await?;
    drain_after_shutdown(in_flight, &cfg.server).await;
    Ok(())
}

// ── Dual-transport path (Linux + vsock-listener feature) ─────────────────────

/// Run Unix and vsock listeners concurrently.
///
/// A [`tokio::sync::Notify`] fans out the single `shutdown_rx` signal to both
/// listeners: when `shutdown_rx` fires the notifier wakes a background task
/// that sends `()` on both per-listener oneshots.
#[cfg(all(target_os = "linux", feature = "vsock-listener"))]
async fn serve_unix_and_vsock(
    cfg: &crate::config::SomaConfig,
    vsock_cfg: &crate::config::VsockConfig,
    router: tonic::transport::server::Router,
    shutdown_rx: oneshot::Receiver<()>,
) -> SomaResult<()> {
    use tokio::sync::Notify;

    // Shared notifier: fires when the original shutdown_rx completes.
    let shutdown = Arc::new(Notify::new());

    // Watch the parent oneshot in a background task and notify when done.
    {
        let shutdown = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let _ = shutdown_rx.await;
            shutdown.notify_waiters();
        });
    }

    // Each listener gets its own oneshot, both fed by the Notify.
    let (unix_tx, unix_rx) = oneshot::channel::<()>();
    let (vsock_tx, vsock_rx) = oneshot::channel::<()>();
    {
        let shutdown = Arc::clone(&shutdown);
        tokio::spawn(async move {
            shutdown.notified().await;
            // Ignore send errors — listener may have already exited.
            let _ = unix_tx.send(());
            let _ = vsock_tx.send(());
        });
    }

    // Router is Clone in tonic 0.14 — each listener gets its own copy.
    let unix_fut = unix::serve(&cfg.server, router.clone(), unix_rx);
    let vsock_fut = vsock::serve(vsock_cfg, router, vsock_rx);

    // Run both concurrently; surface the first error.
    let (unix_result, vsock_result) = tokio::join!(unix_fut, vsock_fut);
    unix_result?;
    vsock_result?;
    Ok(())
}

// ── Shared drain helper ───────────────────────────────────────────────────────

async fn drain_after_shutdown(
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
    server_cfg: &crate::config::ServerConfig,
) {
    let deadline = Duration::from_secs(server_cfg.drain_secs);
    match crate::shutdown::drain_in_flight(in_flight, deadline).await {
        Ok(()) => tracing::info!("drain complete — all in-flight dispatches finished"),
        Err(remaining) => tracing::warn!(
            remaining,
            "drain deadline elapsed with in-flight dispatches still active"
        ),
    }
}
