//! Bootstrap — wires config → TLS bind → upstream lifed channel → router.
//!
//! Sub-phase B (Spec C₃ §5 + §12.B) extends Sub-phase A's wiring with:
//! - **Real Tier-1 verifier installation** — `dev_signer::install_tier1_verifier`
//!   pins a `JwksCache` constructed from `cfg.auth.jwks_url`. When
//!   `cfg.auth.dev_signer_enabled` is true, the verifier is built via
//!   `JwksCache::dev_only` so the magic `Bearer dev-token-for-{user_id}`
//!   shortcut keeps working.
//! - **KMS provider abstraction** — Sub-phase A's concrete `Keystore`
//!   handle is replaced by an `Arc<dyn KmsSigner>`. Production builds
//!   resolve `cfg.auth.kms_provider` to a Vault/AWS/GCP signer; dev
//!   uses `StaticKeystore::generate_dev`.
//! - **JWKS publish** — `bootstrap` writes the active signer's JWKS to
//!   `cfg.auth.publish_jwks_path` atomically (write-tmp + rename) so
//!   downstream verifiers (lifed) can pick up rotation without races.
//!
//! The HTTP entrypoints, listener glue, and signal handler stay
//! unchanged from Sub-phase A.

use std::path::Path;
use std::sync::Arc;

use futures::Stream;
use futures::stream::unfold;
use tokio::sync::oneshot;
use tonic::transport::Server;
use tonic_web::GrpcWebLayer;

use life_runtime_proto::life::v1 as pb;

use crate::auth::dev_signer;
use crate::auth::jwks::{JwksCache, JwksCacheConfig, JwksSource};
use crate::auth::kms::{KmsSigner, StaticKeystore};
use crate::auth::middleware::AuthLayer;
use crate::auth::tier2::Tier2Minter;
use crate::config::{AuthConfig, KmsProvider, LifegwConfig};
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
        "lifegw starting (Sub-phase B)",
    );

    let shutdown_rx = crate::shutdown::install_signal_handler();
    let bind = listener::bind(&cfg.tls, &cfg.listen).await?;
    serve_with_listener(cfg, bind, shutdown_rx).await
}

/// Serve atop an already-bound `TlsBind`. Useful for integration tests
/// that pre-bind a listener so they can extract the local port before
/// launching the server. Resolves the KMS signer from
/// `cfg.auth.kms_provider`.
pub async fn serve_with_listener(
    cfg: LifegwConfig,
    bind: TlsBind,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifegwResult<()> {
    let signer = build_signer(&cfg.auth)?;
    serve_with_listener_and_signer(cfg, bind, signer, shutdown_rx).await
}

/// Serve atop an already-bound `TlsBind` with a pre-constructed signer.
/// Used by integration tests that need the gateway and the conformance
/// reader to share key material via the published JWKS file.
pub async fn serve_with_listener_and_signer(
    cfg: LifegwConfig,
    bind: TlsBind,
    signer: Arc<dyn KmsSigner>,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifegwResult<()> {
    install_default_crypto_provider();

    // Tier-1 verifier — install before the auth Layer goes live.
    install_tier1_verifier(&cfg.auth)?;

    // JWKS publish — write the signer's public key set to the
    // configured path atomically so downstream verifiers (lifed) can
    // pick it up.
    if let Some(path) = cfg.auth.publish_jwks_path.as_ref() {
        publish_jwks_atomic(path, &*signer)?;
        tracing::info!(path = %path.display(), "published lifegw JWKS");
    }

    let upstream_path = Arc::new(cfg.upstream.lifed_uds_path.clone());
    let upstream_channel = connect_uds(&cfg.upstream.lifed_uds_path).await?;

    let minter = Arc::new(Tier2Minter::new(signer, &cfg.auth));
    let auth_layer = AuthLayer::new(
        minter,
        cfg.auth.dev_signer_enabled,
        Arc::clone(&upstream_path),
    );

    let agent = AgentForwarder::new(upstream_channel.clone());
    let events = EventsForwarder::new(upstream_channel.clone());
    let wallet = WalletForwarder::new(upstream_channel.clone());
    let identity = IdentityForwarder::new(upstream_channel.clone());

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

/// Sub-phase A back-compat wrapper. Accepts a [`Keystore`] and wraps it
/// in a [`StaticKeystore`] signer. Existing tests that hand-pinned a
/// keystore continue to work without changes.
#[doc(hidden)]
pub async fn serve_with_listener_and_keystore(
    cfg: LifegwConfig,
    bind: TlsBind,
    keystore: crate::auth::keystore::Keystore,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifegwResult<()> {
    let signer: Arc<dyn KmsSigner> = Arc::new(StaticKeystore::from_keystore(keystore));
    serve_with_listener_and_signer(cfg, bind, signer, shutdown_rx).await
}

/// Resolve the configured KMS provider into a concrete [`KmsSigner`]
/// trait object.
fn build_signer(cfg: &AuthConfig) -> LifegwResult<Arc<dyn KmsSigner>> {
    match cfg.kms_provider {
        KmsProvider::Dev => Ok(Arc::new(StaticKeystore::generate_dev()?)),
        #[cfg(feature = "kms-vault")]
        KmsProvider::Vault => match cfg.vault.as_ref() {
            Some(v) => Ok(Arc::new(crate::auth::kms::VaultTransit::new(
                v.addr.clone(),
                v.token.clone(),
                v.key_name.clone(),
                v.kid.clone(),
            )?)),
            None => Err(LifegwError::Config(
                "auth.kms_provider = vault but [auth.vault] missing".to_string(),
            )),
        },
        #[cfg(not(feature = "kms-vault"))]
        KmsProvider::Vault => Err(LifegwError::Config(
            "auth.kms_provider = vault but lifegw built without `kms-vault` feature".to_string(),
        )),
        #[cfg(feature = "kms-aws")]
        KmsProvider::Aws => Err(LifegwError::Auth(
            "kms-aws provider configured but body deferred to Sub-phase E".to_string(),
        )),
        #[cfg(not(feature = "kms-aws"))]
        KmsProvider::Aws => Err(LifegwError::Config(
            "auth.kms_provider = aws but lifegw built without `kms-aws` feature".to_string(),
        )),
        #[cfg(feature = "kms-gcp")]
        KmsProvider::Gcp => Err(LifegwError::Auth(
            "kms-gcp provider configured but body deferred to Sub-phase E".to_string(),
        )),
        #[cfg(not(feature = "kms-gcp"))]
        KmsProvider::Gcp => Err(LifegwError::Config(
            "auth.kms_provider = gcp but lifegw built without `kms-gcp` feature".to_string(),
        )),
    }
}

/// Install the global Tier-1 verifier from `cfg.auth`.
fn install_tier1_verifier(cfg: &AuthConfig) -> LifegwResult<()> {
    let cache = if cfg.dev_signer_enabled {
        // Dev path — accept the magic Bearer shortcut for tests / CI.
        Arc::new(JwksCache::dev_only())
    } else {
        // Production path — fetch JWKS from the configured URL.
        let mut jwks_cfg = JwksCacheConfig::new(
            JwksSource::Url(cfg.jwks_url.clone()),
            cfg.tier1_audience.clone(),
            cfg.tier1_issuer.clone(),
        );
        jwks_cfg.ttl = cfg.jwks_cache_ttl;
        jwks_cfg.rotation_grace = cfg.jwks_rotation_grace;
        Arc::new(JwksCache::new(jwks_cfg))
    };
    dev_signer::install_tier1_verifier(cache);
    Ok(())
}

/// Atomic JWKS publish — write to a temporary file then rename into
/// place so concurrent readers (lifed) never see a partial document.
///
/// Per Spec C₃ §5: the JWKS contains the current key plus any
/// retired-in-grace keys returned by `signer.publish_jwks()` so
/// in-flight tokens minted under a previous key continue to verify.
fn publish_jwks_atomic(path: &Path, signer: &dyn KmsSigner) -> LifegwResult<()> {
    use std::io::Write;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            LifegwError::Auth(format!("create jwks parent dir {}: {e}", parent.display()))
        })?;
    }
    let jwks = signer.publish_jwks();
    let body = serde_json::to_vec_pretty(&jwks)
        .map_err(|e| LifegwError::Auth(format!("serialize jwks: {e}")))?;

    // tempfile::NamedTempFile::persist() handles the atomic rename.
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let mut tmp = match parent {
        Some(p) => tempfile::NamedTempFile::new_in(p),
        None => tempfile::NamedTempFile::new(),
    }
    .map_err(|e| LifegwError::Auth(format!("open jwks tmp: {e}")))?;
    tmp.write_all(&body)
        .map_err(|e| LifegwError::Auth(format!("write jwks tmp: {e}")))?;
    tmp.flush()
        .map_err(|e| LifegwError::Auth(format!("flush jwks tmp: {e}")))?;
    tmp.persist(path)
        .map_err(|e| LifegwError::Auth(format!("persist jwks {}: {e}", path.display())))?;
    Ok(())
}

/// Convert a TCP listener + TLS acceptor into a `Stream` of accepted
/// TLS connections that tonic's `serve_with_incoming_shutdown` can
/// consume.
///
/// Each yielded item is a `LifegwTlsStream` so tonic 0.14's `Connected`
/// trait bound is satisfied. Errors during TCP accept or TLS handshake
/// are logged and skipped — a single misbehaving client never tears
/// down the listener.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn publish_jwks_atomic_writes_valid_doc() {
        let signer: Arc<dyn KmsSigner> =
            Arc::new(StaticKeystore::generate_dev().expect("keystore"));
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("jwks.json");

        publish_jwks_atomic(&path, &*signer).expect("publish");
        assert!(path.exists());

        let body = std::fs::read_to_string(&path).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
        let keys = parsed["keys"].as_array().expect("keys array");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0]["alg"], serde_json::json!("ES256"));
        assert_eq!(keys[0]["kid"], serde_json::json!(signer.active_kid()));
    }

    #[test]
    fn publish_jwks_atomic_overwrites_existing() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("jwks.json");
        std::fs::write(&path, "stale").expect("write stale");

        let signer: Arc<dyn KmsSigner> =
            Arc::new(StaticKeystore::generate_dev().expect("keystore"));
        publish_jwks_atomic(&path, &*signer).expect("publish over stale");

        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains("\"kid\""));
        assert_ne!(body, "stale");
    }
}
