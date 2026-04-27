//! Signal-handling and graceful drain orchestrator.
//!
//! Mirrors the lifed pattern: a SIGTERM/SIGINT handler that fires a
//! `oneshot::Receiver<()>` once. Sub-phase D wires the WS-drain semantics
//! (close existing WS with code 1001 going-away). Sub-phase A simply waits
//! for in-flight unary requests to drain via tonic's `serve_with_incoming_shutdown`.

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;

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

/// Build the shutdown-signal future the tonic server consumes.
pub async fn shutdown_signal(rx: oneshot::Receiver<()>) {
    let _ = rx.await;
}
