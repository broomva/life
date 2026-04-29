//! Signal-handling and graceful drain orchestrator.
//!
//! Mirrors the lifed pattern: a SIGTERM/SIGINT handler that fires a
//! `oneshot::Receiver<()>` once. Sub-phase D adds a SIGHUP handler
//! that drives the cert reloader (Spec C₃ §4.3 LOCKED L4-D10).

use std::sync::Arc;

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;

use crate::services::cert_watch::CertReloader;

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

/// Install a SIGHUP handler that triggers an immediate cert reload via
/// the supplied [`CertReloader`]. Sub-phase D (D3).
///
/// SIGHUP loops forever — `systemctl reload lifegw.service` translates
/// to repeated SIGHUPs across cert rotations. The handler logs the
/// outcome of each reload so operators can correlate `journalctl` lines
/// with `systemctl reload` invocations.
pub fn install_sighup_handler(reloader: Arc<CertReloader>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut sighup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "failed to install SIGHUP handler");
                return;
            }
        };
        loop {
            if sighup.recv().await.is_none() {
                return;
            }
            tracing::info!("SIGHUP received — reloading TLS certificates");
            match reloader.reload() {
                Ok(n) => tracing::info!(cert_count = n, "cert reload succeeded"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "cert reload rejected; previous config stays live"
                ),
            }
        }
    })
}

/// Build the shutdown-signal future the tonic server consumes.
pub async fn shutdown_signal(rx: oneshot::Receiver<()>) {
    let _ = rx.await;
}
