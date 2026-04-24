//! Transport dialers for `LifeClient`.
//!
//! Unix socket is the primary production transport. vsock and TCP are
//! feature-gated — TCP is dev-only; production deployments always go
//! over Unix or vsock.
//!
//! Implementation note: tonic 0.14 is built on hyper 1.x, so a custom
//! connector must wrap its `tokio::io::{AsyncRead, AsyncWrite}` stream
//! in `hyper_util::rt::TokioIo` before tonic can consume it.

use crate::error::{LifeClientError, LifeResult};
#[cfg(feature = "tcp")]
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

/// Transport selector for connecting to a `lifed` instance.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum LifeTransport {
    /// Unix domain socket (primary production transport).
    Unix(PathBuf),
    /// vsock transport (VM guest ↔ host).
    #[cfg(feature = "vsock")]
    Vsock {
        /// Context ID of the target VM.
        cid: u32,
        /// Port on the target VM.
        port: u32,
    },
    /// TCP (development only).
    #[cfg(feature = "tcp")]
    Tcp(SocketAddr),
}

/// Entry point for the Life Kernel client. Holds a multiplexed tonic
/// `Channel`; service handles are obtained via the factory methods on
/// this struct.
#[derive(Clone)]
pub struct LifeClient {
    pub(crate) channel: Channel,
}

impl LifeClient {
    /// Connect to a `lifed` instance over the given transport.
    pub async fn connect(transport: LifeTransport) -> LifeResult<Self> {
        let channel = match transport {
            LifeTransport::Unix(path) => {
                let path = path.clone();
                Endpoint::try_from("http://[::]:0")
                    .map_err(|e| LifeClientError::Transport(e.to_string()))?
                    .connect_with_connector(service_fn(move |_: Uri| {
                        let path = path.clone();
                        async move {
                            UnixStream::connect(path)
                                .await
                                .map(hyper_util::rt::TokioIo::new)
                        }
                    }))
                    .await
                    .map_err(|e| LifeClientError::Transport(e.to_string()))?
            }
            #[cfg(feature = "vsock")]
            LifeTransport::Vsock { cid, port } => {
                Endpoint::try_from("http://[::]:0")
                    .map_err(|e| LifeClientError::Transport(e.to_string()))?
                    .connect_with_connector(service_fn(move |_: Uri| async move {
                        tokio_vsock::VsockStream::connect(tokio_vsock::VsockAddr::new(cid, port))
                            .await
                            .map(hyper_util::rt::TokioIo::new)
                    }))
                    .await
                    .map_err(|e| LifeClientError::Transport(e.to_string()))?
            }
            #[cfg(feature = "tcp")]
            LifeTransport::Tcp(addr) => Endpoint::try_from(format!("http://{addr}"))
                .map_err(|e| LifeClientError::Transport(e.to_string()))?
                .connect()
                .await
                .map_err(|e| LifeClientError::Transport(e.to_string()))?,
        };
        Ok(Self { channel })
    }

    /// Access the underlying tonic `Channel` — used by service handles
    /// to construct the generated client stubs.
    pub(crate) fn channel(&self) -> Channel {
        self.channel.clone()
    }
}
