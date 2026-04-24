//! Transport multiplexer.
//!
//! Phase 2 ships only the Unix-socket listener. Vsock support lands in BRO-897
//! behind the `vsock-listener` feature on Linux. Both listeners share a single
//! tonic `Router` and a single shutdown `oneshot::Receiver<()>`.

pub mod unix;

use std::sync::Arc;

use aios_protocol::ports::KernelPort;
use tokio::sync::oneshot;
use tonic::transport::Server;

use crate::config::LifedConfig;
use crate::error::LifedResult;
use crate::server::LifeKernelService;

/// Spawns every configured listener and awaits shutdown.
///
/// Returns `Ok(())` on graceful shutdown. Any listener error bubbles up as
/// `LifedError::Server(..)`.
pub async fn serve<E: KernelPort + 'static>(
    cfg: &LifedConfig,
    engine: Arc<E>,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    let service = LifeKernelService::new(engine).into_server();
    let router = Server::builder().add_service(service);

    unix::serve(&cfg.server, router, shutdown_rx).await
    // BRO-897 wires the vsock listener here when cfg.server.vsock.is_some().
}
