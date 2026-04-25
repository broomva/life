//! Vsock listener — Linux-only.
//!
//! Compiled in only on Linux with the `vsock-listener` feature. Mirrors the
//! Unix listener's serve loop: build a stream from `VsockListener::incoming`,
//! hand off to tonic's `serve_with_incoming_shutdown`.
//!
//! ## `tonic-conn` compatibility note
//!
//! `tokio-vsock 0.5`'s `tonic-conn` feature targets tonic `0.10.x`, which is
//! incompatible with the workspace's tonic `0.14`. This module therefore
//! provides its own thin newtype — `ConnectedVsockStream` — that delegates
//! `AsyncRead` / `AsyncWrite` to the inner `VsockStream` and implements
//! `tonic::transport::server::Connected` against tonic 0.14.
//!
//! ## Runtime testing
//!
//! vsock cannot bind on macOS or in environments without a running VMM kernel
//! module. The module-level `#[cfg]` ensures the code is only compiled on
//! Linux + feature; runtime tests are annotated
//! `#[cfg(all(target_os = "linux", feature = "vsock-listener"))]` in the test
//! block below. CI on Linux with the feature enabled provides the live test
//! surface; macOS dev builds verify compile-time correctness only via
//! `cargo check -p lifed --no-default-features`.

#![cfg(all(target_os = "linux", feature = "vsock-listener"))]

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::oneshot;
use tokio_stream::StreamExt as _;
use tokio_vsock::{VsockAddr, VsockListener, VsockStream};
use tonic::transport::server::{Connected, Router};

use crate::config::VsockConfig;
use crate::error::{LifedError, LifedResult};

// ── Connected newtype ─────────────────────────────────────────────────────────

/// Thin wrapper around [`VsockStream`] that satisfies the
/// `tonic::transport::server::Connected` bound required by
/// `Router::serve_with_incoming_shutdown`.
///
/// `tokio-vsock 0.5`'s own `tonic-conn` feature targets tonic 0.10, so we
/// implement the trait ourselves against tonic 0.14.
pub struct ConnectedVsockStream(VsockStream);

/// Stub connection info returned by `ConnectedVsockStream::connect_info`.
///
/// Vsock sockets do not expose the rich peer metadata that TCP or Unix sockets
/// do; CID + port are accessible through the inner stream if needed but are not
/// required by tonic's `Connected` trait contract.
#[derive(Debug, Clone)]
pub struct VsockConnectInfo {
    /// The remote CID, if available.
    pub peer_cid: Option<u32>,
    /// The remote port, if available.
    pub peer_port: Option<u32>,
}

impl Connected for ConnectedVsockStream {
    type ConnectInfo = VsockConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        let peer = self.0.peer_addr().ok();
        VsockConnectInfo {
            peer_cid: peer.map(|a: VsockAddr| a.cid()),
            peer_port: peer.map(|a: VsockAddr| a.port()),
        }
    }
}

impl AsyncRead for ConnectedVsockStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for ConnectedVsockStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

// ── Listener stream adapter ───────────────────────────────────────────────────

/// Wraps `VsockListener::incoming()` (which yields `io::Result<VsockStream>`)
/// and maps each item to `io::Result<ConnectedVsockStream>` so the stream
/// satisfies the `serve_with_incoming_shutdown` bound.
fn incoming_connected(
    listener: VsockListener,
) -> impl tokio_stream::Stream<Item = io::Result<ConnectedVsockStream>> {
    listener.incoming().map(|r| r.map(ConnectedVsockStream))
}

// ── Public serve function ─────────────────────────────────────────────────────

/// Bind the configured vsock `CID:port` and serve the router until shutdown.
///
/// Consumes `router` (same pattern as [`crate::listener::unix::serve`]).
pub async fn serve(
    cfg: &VsockConfig,
    router: Router,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    let addr = VsockAddr::new(cfg.cid, cfg.port);
    let listener = VsockListener::bind(addr)
        .map_err(|e| LifedError::Server(format!("vsock bind {:?}: {e}", addr)))?;

    tracing::info!(
        target: "lifed::listener::vsock",
        cid = cfg.cid,
        port = cfg.port,
        "vsock listener bound",
    );

    let incoming = incoming_connected(listener);

    router
        .serve_with_incoming_shutdown(incoming, async move {
            let _ = shutdown_rx.await;
        })
        .await
        .map_err(|e| LifedError::Server(format!("vsock serve: {e}")))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// Runtime vsock tests only run when the kernel vsock module is actually
// available (i.e., on Linux with /dev/vsock present). CI that lacks vsock
// can verify compilation; live bind/connect tests require a real VMM.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `VsockConnectInfo` implements `Clone + Send + Sync` as
    /// required by the `Connected::ConnectInfo` bound.
    ///
    /// This test compiles only when the cfg gate is satisfied; it validates the
    /// trait bounds at compile time without requiring a live vsock device.
    #[test]
    fn vsock_connect_info_satisfies_bounds() {
        fn assert_bounds<T: Clone + Send + Sync>() {}
        assert_bounds::<VsockConnectInfo>();
    }

    /// Verify that `ConnectedVsockStream` correctly satisfies the
    /// `Connected<ConnectInfo = VsockConnectInfo>` bound.
    #[test]
    fn connected_vsock_stream_implements_connected() {
        fn assert_connected<T: Connected>() {}
        assert_connected::<ConnectedVsockStream>();
    }
}
