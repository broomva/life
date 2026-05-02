//! Admin custody-oracle UDS listener (Spec D D-Sub-E).
//!
//! Mirrors the lifegw `AdminAcceptor`/`AdminConn` shape. The acceptor
//! yields `AdminConn`s that carry the connecting peer's credentials so
//! the [`crate::admin::CustodyOracleService`] handlers can authorise
//! per [`crate::admin::policy::AdminPolicy`].

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

use crate::admin::peercred::{self, PeerCred};
use crate::config::AdminPlaneConfig;
use crate::error::{SomaError, SomaResult};

/// Bind the configured admin UDS, apply mode/group settings, and
/// return an [`AdminAcceptor`] that yields [`AdminConn`] values for
/// tonic to consume via `serve_with_incoming_shutdown`.
pub async fn bind(cfg: &AdminPlaneConfig) -> SomaResult<AdminAcceptor> {
    prepare_socket_path(&cfg.unix_socket)?;
    let listener = UnixListener::bind(&cfg.unix_socket)
        .map_err(|e| SomaError::Server(format!("bind admin {}: {e}", cfg.unix_socket.display())))?;
    if let Some(mode) = cfg.unix_socket_mode {
        std::fs::set_permissions(&cfg.unix_socket, std::fs::Permissions::from_mode(mode))
            .map_err(|e| SomaError::Server(format!("chmod admin: {e}")))?;
    }
    if let Some(group) = cfg.unix_socket_group.as_deref() {
        tracing::warn!(
            target: "soma::admin::listener",
            group,
            path = %cfg.unix_socket.display(),
            "unix_socket_group honored via systemd SocketGroup; \
             in-process chown deferred (matches lifegw + lifed pattern)",
        );
    }
    Ok(AdminAcceptor { inner: listener })
}

/// Build the shutdown-signal future the tonic server consumes.
pub async fn shutdown_signal(rx: oneshot::Receiver<()>) {
    let _ = rx.await;
}

fn prepare_socket_path(path: &Path) -> SomaResult<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| SomaError::Server(format!("create {}: {e}", parent.display())))?;
    }
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| SomaError::Server(format!("unlink stale admin: {e}")))?;
    }
    Ok(())
}

/// Acceptor that yields [`AdminConn`]s — each carrying the connected
/// peer's `PeerCred` so admin handlers can run policy checks.
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

/// One accepted admin-plane connection, decorated with its peer creds.
pub struct AdminConn {
    stream: UnixStream,
    pub cred: PeerCred,
}

/// Connection info exposed via tonic's `Connected` trait. Lands in the
/// per-request `Extensions` so handlers can extract it via
/// [`CustodyOracleService::cred`].
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
