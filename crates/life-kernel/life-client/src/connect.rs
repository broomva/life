//! Transport dialer for the Life Kernel wire surface.
//!
//! Real transports (Unix, vsock) land in Task 15. This module contains
//! scaffold stubs that make the public API resolve.

use crate::error::{LifeClientError, LifeResult};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Entry point for the Life Kernel client.
///
/// Placeholder; Task 15 rewires this.
pub struct LifeClient;

/// Transport selector for connecting to a `lifed` instance.
#[non_exhaustive]
pub enum LifeTransport {
    /// Unix domain socket (default for local deployments).
    Unix(PathBuf),
    /// VSock transport (VM guest ↔ host).
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

impl LifeClient {
    /// Connect to a `lifed` instance over the given transport.
    ///
    /// Placeholder; Task 15 rewires this.
    pub async fn connect(_transport: LifeTransport) -> LifeResult<Self> {
        Err(LifeClientError::Transport("scaffold stub".into()))
    }
}
