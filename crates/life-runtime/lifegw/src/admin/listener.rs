//! Admin-plane UDS listener for lifegw. Sub-phase D (D2).
//!
//! Mirrors the lifed pattern at
//! `crates/life-runtime/lifed/src/listener/admin.rs`: bind a Unix
//! domain socket, apply mode/group, wrap each accepted stream in an
//! `AdminConn` carrying the peer credentials so the gateway-admin
//! handlers can authorise per [`crate::admin::policy::AdminPolicy`].

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

use crate::admin::peercred::{self, PeerCred};
use crate::config::AdminPlaneConfig;
use crate::error::{LifegwError, LifegwResult};

/// Bind the configured admin UDS, apply mode/group settings, and
/// return an `AdminAcceptor` that yields `AdminConn` values for tonic
/// to consume via `serve_with_incoming_shutdown`.
pub async fn bind(cfg: &AdminPlaneConfig) -> LifegwResult<AdminAcceptor> {
    prepare_socket_path(&cfg.unix_socket)?;
    let listener = UnixListener::bind(&cfg.unix_socket).map_err(|e| {
        LifegwError::Listener(format!("bind admin {}: {e}", cfg.unix_socket.display()))
    })?;
    if let Some(mode) = cfg.unix_socket_mode {
        std::fs::set_permissions(&cfg.unix_socket, std::fs::Permissions::from_mode(mode))
            .map_err(|e| LifegwError::Listener(format!("chmod admin: {e}")))?;
    }
    if let Some(group) = cfg.unix_socket_group.as_deref() {
        tracing::warn!(
            target: "lifegw::admin::listener",
            group,
            path = %cfg.unix_socket.display(),
            "unix_socket_group honored via systemd SocketGroup; \
             in-process chown deferred (matches lifed pattern)",
        );
    }
    Ok(AdminAcceptor { inner: listener })
}

/// Build the shutdown-signal future the tonic server consumes.
pub async fn shutdown_signal(rx: oneshot::Receiver<()>) {
    let _ = rx.await;
}

fn prepare_socket_path(path: &Path) -> LifegwResult<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| LifegwError::Listener(format!("create {}: {e}", parent.display())))?;
    }
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| LifegwError::Listener(format!("unlink stale admin: {e}")))?;
    }
    Ok(())
}

/// Accept loop that wraps each connection in an `AdminConn` carrying
/// the peer credentials.
pub struct AdminAcceptor {
    inner: UnixListener,
}

impl futures::Stream for AdminAcceptor {
    type Item = std::io::Result<AdminConn>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let acc = self.get_mut();
        match acc.inner.poll_accept(cx) {
            Poll::Ready(Ok((stream, _addr))) => {
                let cred = peercred::peer_cred(&stream).unwrap_or(PeerCred {
                    pid: 0,
                    uid: 0,
                    gid: 0,
                });
                Poll::Ready(Some(Ok(AdminConn { stream, cred })))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// One accepted admin-plane UDS connection.
pub struct AdminConn {
    stream: UnixStream,
    pub cred: PeerCred,
}

/// Connection info exposed via tonic's `Connected` trait.
#[derive(Debug, Clone, Copy)]
pub struct AdminConnInfo {
    pub cred: PeerCred,
}

impl tonic::transport::server::Connected for AdminConn {
    type ConnectInfo = AdminConnInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        AdminConnInfo { cred: self.cred }
    }
}

impl AsyncRead for AdminConn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for AdminConn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}
