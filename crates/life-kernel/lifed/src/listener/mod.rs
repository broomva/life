//! Transport multiplexer.
//!
//! Phase 2 ships only the Unix-socket listener. Vsock support lands in BRO-897
//! behind the `vsock-listener` feature on Linux. Both listeners share a single
//! tonic `Router` and a single shutdown `oneshot::Receiver<()>`.

pub mod unix;

use std::sync::Arc;
use std::time::Duration;

use aios_protocol::hypervisor::VmHandle;
use aios_protocol::ports::KernelPort;
use tokio::sync::oneshot;
use tonic::transport::Server;

use crate::config::LifedConfig;
use crate::error::LifedResult;
use crate::server::LifeKernelService;

/// Spawns every configured listener and awaits shutdown + in-flight drain.
///
/// `seed` is passed to [`LifeKernelService::with_seed`] to populate the
/// live-VM index from the replay state recorded at bootstrap.  Pass
/// `Vec::new()` for tests that do not need seeding.
///
/// Returns `Ok(())` on graceful shutdown. Any listener error bubbles up as
/// `LifedError::Server(..)`.
pub async fn serve<E: KernelPort + 'static>(
    cfg: &LifedConfig,
    engine: Arc<E>,
    shutdown_rx: oneshot::Receiver<()>,
    seed: Vec<VmHandle>,
) -> LifedResult<()> {
    let service = LifeKernelService::with_seed(engine, seed);
    let in_flight = service.in_flight();

    let router = Server::builder().add_service(service.into_server());
    unix::serve(&cfg.server, router, shutdown_rx).await?;

    // Drain in-flight dispatches before returning to the caller (main.rs).
    // The tonic server has already stopped accepting new connections by the
    // time we reach this line (shutdown_rx fired), so the counter can only
    // go down from here.
    let deadline = Duration::from_secs(cfg.server.drain_secs);
    match crate::shutdown::drain_in_flight(in_flight, deadline).await {
        Ok(()) => tracing::info!("drain complete — all in-flight dispatches finished"),
        Err(remaining) => tracing::warn!(
            remaining,
            "drain deadline elapsed with in-flight dispatches still active"
        ),
    }

    Ok(())
    // BRO-897 wires the vsock listener here when cfg.server.vsock.is_some().
}
