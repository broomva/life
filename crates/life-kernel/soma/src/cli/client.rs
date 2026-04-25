//! Unix-socket tonic client for the soma daemon.
//!
//! Builds a tonic [`Channel`] connected to the daemon's Unix domain socket
//! using a `tower::service_fn` connector + `hyper_util::rt::TokioIo`.
//! The dummy HTTP URI (`http://[::]:50051`) satisfies tonic's [`Endpoint`]
//! requirement but is never actually used — the underlying stream is a local
//! `UnixStream`.

use std::path::Path;

use anyhow::{Context, Result};
use hyper_util::rt::TokioIo;
use life_kernel_proto::pb::kernel_service_client::KernelServiceClient;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

/// Connect to the soma daemon over its Unix socket and return a
/// [`KernelServiceClient`] ready for RPC calls.
pub async fn connect(socket: &Path) -> Result<KernelServiceClient<Channel>> {
    let socket_path = socket.to_path_buf();

    // Tonic requires an http(s)://… URI even for Unix transport; the URI is
    // never used because the connector overrides the actual stream.
    let endpoint =
        Endpoint::try_from("http://[::]:50051").context("constructing tonic endpoint")?;

    let channel = endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = socket_path.clone();
            async move { UnixStream::connect(&path).await.map(TokioIo::new) }
        }))
        .await
        .context("connecting to soma unix socket")?;

    Ok(KernelServiceClient::new(channel))
}
