//! Bootstrap — wires config → TLS bind → upstream lifed channel → router.
//!
//! Sub-phase A scope (Spec C₃ §12.A): the gateway terminates TLS, runs the
//! auth middleware (dev-signer Tier-1 → Tier-2 mint), forwards the four
//! `life.v1.*` services to lifed via UDS, and answers `/healthz` without
//! auth. WS upgrade is C; rate-limit is D; production KMS is E.
//!
//! Two entrypoints:
//! - `run_daemon` — production path (binds the configured TCP address).
//! - `serve_with_listener` — used by integration tests to pre-bind on
//!   `127.0.0.1:0`, extract the resolved port, and start serving.

use std::path::Path;
use std::sync::Arc;

use futures::Stream;
use futures::stream::unfold;
use tokio::sync::oneshot;
use tonic::transport::Server;
use tonic_web::GrpcWebLayer;

use life_runtime_proto::life::v1 as pb;

use crate::auth::keystore::Keystore;
use crate::auth::middleware::AuthLayer;
use crate::auth::tier2::Tier2Minter;
use crate::config::LifegwConfig;
use crate::error::{LifegwError, LifegwResult};
use crate::listener::{self, LifegwTlsStream, TlsBind};
use crate::proxy::{
    AgentForwarder, EventsForwarder, IdentityForwarder, WalletForwarder, connect_uds,
};

/// Production daemon entrypoint. Reads the config (or defaults), installs
/// the signal handler, binds TLS, dials upstream, and serves until SIGTERM.
pub async fn run_daemon(config_path: Option<&Path>) -> LifegwResult<()> {
    let cfg = LifegwConfig::load(config_path)?;
    let _vigil_guard = crate::observability::init(&cfg.observability)?;
    install_default_crypto_provider();

    tracing::info!(
        https_addr = %cfg.listen.https_addr,
        upstream = %cfg.upstream.lifed_uds_path.display(),
        dev_signer = cfg.auth.dev_signer_enabled,
        "lifegw starting (Sub-phase A)",
    );

    let shutdown_rx = crate::shutdown::install_signal_handler();
    let bind = listener::bind(&cfg.tls, &cfg.listen).await?;
    serve_with_listener(cfg, bind, shutdown_rx).await
}

/// Serve atop an already-bound `TlsBind`. Useful for integration tests that
/// pre-bind a listener so they can extract the local port before launching
/// the server. Generates a fresh dev keystore at startup; tests that need
/// to share the keystore with lifed (so the downstream Tier-2 verifier
/// trusts the minted tokens) call [`serve_with_listener_and_keystore`].
pub async fn serve_with_listener(
    cfg: LifegwConfig,
    bind: TlsBind,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifegwResult<()> {
    let keystore = Keystore::generate_dev()?;
    serve_with_listener_and_keystore(cfg, bind, keystore, shutdown_rx).await
}

/// Test helper — same as [`serve_with_listener`] but accepts a pre-generated
/// keystore so callers can publish its JWKS to a path lifed reads.
pub async fn serve_with_listener_and_keystore(
    cfg: LifegwConfig,
    bind: TlsBind,
    keystore: Keystore,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifegwResult<()> {
    install_default_crypto_provider();

    let upstream_path = Arc::new(cfg.upstream.lifed_uds_path.clone());
    let upstream_channel = connect_uds(&cfg.upstream.lifed_uds_path).await?;

    let minter = Arc::new(Tier2Minter::new(keystore, &cfg.auth));
    let auth_layer = AuthLayer::new(
        minter,
        cfg.auth.dev_signer_enabled,
        Arc::clone(&upstream_path),
    );

    let agent = AgentForwarder::new(upstream_channel.clone());
    let events = EventsForwarder::new(upstream_channel.clone());
    let wallet = WalletForwarder::new(upstream_channel.clone());
    let identity = IdentityForwarder::new(upstream_channel.clone());

    // tonic-web `GrpcWebLayer` translates browser fetch (Connect / grpc-web)
    // calls into native gRPC for the underlying tonic services. With
    // `accept_http1(true)` the same listener handles both HTTP/2 (native
    // gRPC) and HTTP/1.1 (browser fetch) connections multiplexed via ALPN.
    let router = Server::builder()
        .accept_http1(true)
        .layer(auth_layer)
        .layer(GrpcWebLayer::new())
        .add_service(pb::agent_server::AgentServer::new(agent))
        .add_service(pb::events_server::EventsServer::new(events))
        .add_service(pb::wallet_server::WalletServer::new(wallet))
        .add_service(pb::identity_server::IdentityServer::new(identity));

    let TlsBind {
        acceptor,
        listener,
        local_addr,
    } = bind;

    tracing::info!(addr = %local_addr, "lifegw listening");

    let incoming = tls_incoming_stream(listener, acceptor);
    router
        .serve_with_incoming_shutdown(incoming, async move {
            let _ = shutdown_rx.await;
        })
        .await
        .map_err(|e| LifegwError::Server(format!("serve_with_incoming_shutdown: {e}")))
}

/// Convert a TCP listener + TLS acceptor into a `Stream` of accepted TLS
/// connections that tonic's `serve_with_incoming_shutdown` can consume.
///
/// Each yielded item is a `LifegwTlsStream` so tonic 0.14's `Connected`
/// trait bound is satisfied. Errors during TCP accept or TLS handshake are
/// logged and skipped — a single misbehaving client never tears down the
/// listener.
fn tls_incoming_stream(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
) -> impl Stream<Item = Result<LifegwTlsStream, std::io::Error>> {
    unfold((listener, acceptor), |(listener, acceptor)| async move {
        loop {
            match listener.accept().await {
                Ok((sock, _peer)) => match acceptor.accept(sock).await {
                    Ok(tls) => {
                        return Some((Ok(LifegwTlsStream::new(tls)), (listener, acceptor)));
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "tls handshake failed");
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "tcp accept failed");
                    return Some((Err(e), (listener, acceptor)));
                }
            }
        }
    })
}

/// Install the rustls default crypto provider exactly once. rustls 0.23
/// requires this dance before any TLS handshake. Multiple calls are
/// harmless — only the first installation has effect.
pub(crate) fn install_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
