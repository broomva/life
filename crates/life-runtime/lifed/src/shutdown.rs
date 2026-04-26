//! Signal-handling and graceful drain orchestrator.

use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;

use crate::error::{LifedError, LifedResult};

/// Install a SIGTERM/SIGINT handler that fires `shutdown_tx` once.
pub fn install_signal_handler() -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "failed to install SIGTERM handler");
                return;
            }
        };
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "failed to install SIGINT handler");
                return;
            }
        };

        tokio::select! {
            _ = sigterm.recv() => tracing::info!("SIGTERM received"),
            _ = sigint.recv()  => tracing::info!("SIGINT received"),
        }
        let _ = tx.send(());
    });

    rx
}

/// Wait up to `deadline` for in-flight calls to drain. Returns the residual
/// inflight count if the deadline elapses.
pub async fn drain_in_flight(
    inflight: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    deadline: Duration,
) -> Result<(), usize> {
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        let count = inflight.load(std::sync::atomic::Ordering::SeqCst);
        if count == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(inflight.load(std::sync::atomic::Ordering::SeqCst))
}

pub async fn force_shutdown_after(
    deadline: Duration,
    inflight: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> LifedResult<()> {
    drain_in_flight(inflight, deadline)
        .await
        .map_err(|residual| LifedError::Shutdown(format!("{residual} inflight at deadline")))
}
