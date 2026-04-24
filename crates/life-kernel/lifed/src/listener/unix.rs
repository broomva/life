//! Unix-socket listener for the tonic `KernelService` router.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::server::Router;

use crate::config::ServerConfig;
use crate::error::{LifedError, LifedResult};

/// Start serving the router over the configured Unix socket and await
/// graceful shutdown.
///
/// The `router` argument is consumed because `Router::serve_with_incoming_shutdown`
/// takes `self` — callers must not re-use it after calling this function.
pub async fn serve(
    cfg: &ServerConfig,
    router: Router,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    prepare_socket_path(&cfg.unix_socket)?;

    let listener = UnixListener::bind(&cfg.unix_socket)
        .map_err(|e| LifedError::Server(format!("bind {}: {e}", cfg.unix_socket.display())))?;

    if let Some(mode) = cfg.unix_socket_mode {
        std::fs::set_permissions(&cfg.unix_socket, std::fs::Permissions::from_mode(mode))
            .map_err(|e| LifedError::Server(format!("chmod socket: {e}")))?;
    }

    // Optional group chown — skip if `cfg.unix_socket_group` is None.
    if let Some(group) = cfg.unix_socket_group.as_deref() {
        chown_socket_group(&cfg.unix_socket, group)?;
    }

    let incoming = UnixListenerStream::new(listener);

    router
        .serve_with_incoming_shutdown(incoming, async move {
            let _ = shutdown_rx.await;
        })
        .await
        .map_err(|e| LifedError::Server(format!("serve: {e}")))
}

/// Remove a stale socket file and ensure the parent directory exists.
fn prepare_socket_path(path: &Path) -> LifedResult<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| LifedError::Server(format!("create {}: {e}", parent.display())))?;
    }
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| LifedError::Server(format!("unlink stale {}: {e}", path.display())))?;
    }
    Ok(())
}

/// Set group ownership on the socket file.
///
/// For Phase 2 MVS, log a warning and skip if `nix` is not a workspace dep.
/// Operators can use systemd `SocketMode`/`SocketGroup` directives until
/// in-process chown lands in a future ticket.
fn chown_socket_group(path: &Path, group: &str) -> LifedResult<()> {
    tracing::warn!(
        target: "lifed::listener::unix",
        group = %group,
        path = %path.display(),
        "unix_socket_group honored via systemd SocketGroup directive; \
         in-process chown deferred to a future ticket (nix not in workspace deps)",
    );
    Ok(())
}
