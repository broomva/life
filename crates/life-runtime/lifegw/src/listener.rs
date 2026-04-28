//! Listener primitives (TLS + TCP bind).
//!
//! Sub-phase A: rustls-backed TLS bind on a configured TCP address.
//! Sub-phase D wires systemd socket activation (`LISTEN_FDS`) so the daemon
//! never calls `bind(2)` in production.

use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tonic::transport::server::{Connected, TcpConnectInfo};

use crate::config::{ListenConfig, TlsConfig};
use crate::error::{LifegwError, LifegwResult};

/// Result of a successful TLS + TCP bind: the rustls acceptor and the
/// bound listener. Bootstrap glues these together with the tonic
/// Server's `serve_with_incoming_shutdown`.
#[non_exhaustive]
pub struct TlsBind {
    pub acceptor: TlsAcceptor,
    pub listener: TcpListener,
    pub local_addr: std::net::SocketAddr,
}

/// Bind a TCP listener at `cfg_listen.https_addr` and load TLS material from
/// `cfg_tls.cert_path` + `cfg_tls.key_path`. Returns the rustls acceptor + the
/// bound listener.
///
/// Used by both production bootstrap and the integration test rig.
pub async fn bind(cfg_tls: &TlsConfig, cfg_listen: &ListenConfig) -> LifegwResult<TlsBind> {
    let acceptor = build_acceptor(&cfg_tls.cert_path, &cfg_tls.key_path)?;
    let listener = TcpListener::bind(&cfg_listen.https_addr)
        .await
        .map_err(|e| LifegwError::Listener(format!("bind {}: {e}", cfg_listen.https_addr)))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| LifegwError::Listener(format!("local_addr: {e}")))?;
    Ok(TlsBind {
        acceptor,
        listener,
        local_addr,
    })
}

/// Build a rustls `TlsAcceptor` from PEM-encoded cert + key files.
pub fn build_acceptor(cert_path: &Path, key_path: &Path) -> LifegwResult<TlsAcceptor> {
    let certs = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;
    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| LifegwError::Tls(format!("server config: {e}")))?;
    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

/// Newtype wrapping `tokio_rustls::server::TlsStream<TcpStream>` so we
/// can impl tonic's `Connected` trait. tonic 0.14 requires the IO type
/// to expose `connect_info` for request extensions; the inner
/// `TcpStream` carries the addresses, so we derive from there.
#[derive(Debug)]
#[non_exhaustive]
pub struct LifegwTlsStream {
    inner: TlsStream<TcpStream>,
    local_addr: Option<SocketAddr>,
    remote_addr: Option<SocketAddr>,
}

impl LifegwTlsStream {
    pub fn new(inner: TlsStream<TcpStream>) -> Self {
        let (tcp, _) = inner.get_ref();
        let local_addr = tcp.local_addr().ok();
        let remote_addr = tcp.peer_addr().ok();
        Self {
            inner,
            local_addr,
            remote_addr,
        }
    }
}

impl Connected for LifegwTlsStream {
    type ConnectInfo = TcpConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        // tonic 0.14's TcpConnectInfo uses public fields; mirror them.
        TcpConnectInfo {
            local_addr: self.local_addr,
            remote_addr: self.remote_addr,
        }
    }
}

impl AsyncRead for LifegwTlsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for LifegwTlsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

fn load_certs(path: &Path) -> LifegwResult<Vec<CertificateDer<'static>>> {
    let pem = std::fs::read(path)
        .map_err(|e| LifegwError::Tls(format!("read cert {}: {e}", path.display())))?;
    let mut reader = std::io::BufReader::new(&pem[..]);
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<_, _>>()
        .map_err(|e| LifegwError::Tls(format!("parse cert: {e}")))?;
    if certs.is_empty() {
        return Err(LifegwError::Tls(format!("no certs in {}", path.display())));
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> LifegwResult<PrivateKeyDer<'static>> {
    let pem = std::fs::read(path)
        .map_err(|e| LifegwError::Tls(format!("read key {}: {e}", path.display())))?;
    let mut reader = std::io::BufReader::new(&pem[..]);
    let key = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| LifegwError::Tls(format!("parse key: {e}")))?
        .ok_or_else(|| LifegwError::Tls(format!("no private key in {}", path.display())))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Generate a self-signed cert via `rcgen`, write cert + key PEMs to
    /// `dir/{cert,key}.pem`, return their paths.
    pub(crate) fn generate_self_signed(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let cert_kp =
            rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
                .expect("rcgen self-signed");
        let cert_pem = cert_kp.cert.pem();
        let key_pem = cert_kp.key_pair.serialize_pem();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, cert_pem).expect("write cert pem");
        std::fs::write(&key_path, key_pem).expect("write key pem");
        (cert_path, key_path)
    }

    #[test]
    fn build_acceptor_round_trip() {
        // rustls in tests requires the default crypto provider be installed once.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = TempDir::new().expect("tempdir");
        let (cert_path, key_path) = generate_self_signed(dir.path());

        let acceptor = build_acceptor(&cert_path, &key_path).expect("build acceptor");
        // Confirm the returned acceptor is usable — `Arc::strong_count` >= 1.
        let _config = acceptor.config().clone();
    }

    #[test]
    fn missing_cert_returns_tls_error() {
        let dir = TempDir::new().expect("tempdir");
        let cert_path = dir.path().join("missing-cert.pem");
        let key_path = dir.path().join("missing-key.pem");
        match build_acceptor(&cert_path, &key_path) {
            Ok(_) => panic!("missing cert path must fail"),
            Err(LifegwError::Tls(_)) => {}
            Err(other) => panic!("expected Tls error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tls_bind_completes_handshake() {
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = TempDir::new().expect("tempdir");
        let (cert_path, key_path) = generate_self_signed(dir.path());
        let cert_pem = std::fs::read(&cert_path).expect("read cert pem");

        let cfg_tls = TlsConfig {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            acme_enabled: false,
        };
        let cfg_listen = ListenConfig {
            https_addr: "127.0.0.1:0".to_string(),
            http_redirect_addr: None,
        };

        let bind_result = bind(&cfg_tls, &cfg_listen).await.expect("bind");
        let local = bind_result.local_addr;
        let acceptor = bind_result.acceptor;
        let listener = bind_result.listener;

        // Server: accept one TLS connection, echo a bytes-string.
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.expect("accept");
            let mut tls = acceptor.accept(sock).await.expect("tls accept");
            tls.write_all(b"OK").await.expect("write");
            let mut buf = [0u8; 4];
            let n = tls.read(&mut buf).await.expect("read");
            buf[..n].to_vec()
        });

        // Client: build a rustls config that trusts the self-signed cert.
        let mut roots = rustls::RootCertStore::empty();
        let mut reader = std::io::BufReader::new(&cert_pem[..]);
        for cert in rustls_pemfile::certs(&mut reader) {
            let cert = cert.expect("parse cert");
            roots.add(cert).expect("add to root store");
        }
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));

        let stream = TcpStream::connect(&local).await.expect("tcp connect");
        let domain = rustls::pki_types::ServerName::try_from("localhost").expect("name");
        let mut tls = connector.connect(domain, stream).await.expect("client tls");

        let mut response = [0u8; 8];
        let n = tls.read(&mut response).await.expect("client read");
        assert_eq!(&response[..n], b"OK");
        tls.write_all(b"PONG").await.expect("client write");
        let received = server.await.expect("server task");
        assert_eq!(&received, b"PONG");
    }
}
