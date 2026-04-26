//! Public-plane UDS listener.
//!
//! Binds `/run/life/life.sock` (configurable via `cfg.public_plane`),
//! chmods to 0660, optionally chowns to `life-runtime`, and serves the
//! tonic Router. Pattern lifted from soma's `listener/unix.rs`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;

use crate::config::PublicPlaneConfig;
use crate::error::{LifedError, LifedResult};

/// Bind the configured public UDS, apply mode/group settings, and return
/// the incoming-stream + shutdown-future tuple that bootstrap can hand to
/// `Server::builder().serve_with_incoming_shutdown(...)`.
///
/// We expose this as a "prepare" helper rather than wrapping the full serve
/// call because tonic 0.14's `Router<L>` is generic over its tower layer
/// stack and propagating those generics through this module adds a lot of
/// trait noise for no real benefit. Bootstrap composes the router (with
/// AuthLayer + future tracing/load-shed) and calls serve directly.
pub async fn bind(cfg: &PublicPlaneConfig) -> LifedResult<UnixListenerStream> {
    prepare_socket_path(&cfg.unix_socket)?;

    let listener = UnixListener::bind(&cfg.unix_socket)
        .map_err(|e| LifedError::Listener(format!("bind {}: {e}", cfg.unix_socket.display())))?;

    if let Some(mode) = cfg.unix_socket_mode {
        std::fs::set_permissions(&cfg.unix_socket, std::fs::Permissions::from_mode(mode))
            .map_err(|e| LifedError::Listener(format!("chmod socket: {e}")))?;
    }

    if let Some(group) = cfg.unix_socket_group.as_deref() {
        tracing::warn!(
            target: "lifed::listener::public",
            group,
            path = %cfg.unix_socket.display(),
            "unix_socket_group honored via systemd SocketGroup directive; \
             in-process chown deferred",
        );
    }

    Ok(UnixListenerStream::new(listener))
}

/// Build the shutdown-signal future the tonic server consumes.
pub fn shutdown_signal(rx: oneshot::Receiver<()>) -> impl std::future::Future<Output = ()> + Send {
    async move {
        let _ = rx.await;
    }
}

fn prepare_socket_path(path: &Path) -> LifedResult<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| LifedError::Listener(format!("create {}: {e}", parent.display())))?;
    }
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| LifedError::Listener(format!("unlink stale {}: {e}", path.display())))?;
    }
    Ok(())
}
