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
use std::time::Duration;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperConnBuilder;
use tokio::sync::oneshot;
use tonic::service::Routes;
use tonic_web::GrpcWebLayer;
use tower::ServiceBuilder;
use tower::ServiceExt;

use life_runtime_proto::life::v1 as pb;

use crate::admin::{
    AdminMetrics, AdminPolicy, Blocklist, CertReloadHook, GatewayAdminService,
    listener as admin_listener,
};
use crate::auth::jwks::{JwksCache, JwksCacheConfig, JwksSource};
use crate::auth::kms::{KmsSigner, StaticKeystore};
use crate::auth::middleware::AuthLayer;
use crate::auth::tier2::Tier2Minter;
use crate::config::{AdminPlaneConfig, AuthConfig, KmsProvider, LifegwConfig};
use crate::error::{LifegwError, LifegwResult};
use crate::listener::{self, TlsBind};
use crate::proxy::{
    AgentForwarder, EventsForwarder, IdentityForwarder, WalletForwarder, connect_uds,
};
use crate::services::cert_watch::CertReloader;
use crate::services::rate_limit::TokenBucketLimiter;
use crate::services::ws::WsLayer;
use life_runtime_proto::life::admin::gw::v1 as admin_pb;

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
        "lifegw starting (Sub-phase D)",
    );

    let shutdown_rx = crate::shutdown::install_signal_handler();

    // Sub-phase D (D3): install SIGHUP handler driving the cert
    // reloader. The reloader is built optimistically — failures are
    // logged but don't block startup (production deploys may have
    // misconfigured paths during rollout; the gateway falls back to
    // the static `bind()` cert in that case).
    if let Ok(reloader) = CertReloader::load(&cfg.tls.cert_path, &cfg.tls.key_path) {
        // Drop the JoinHandle — the SIGHUP task lives for the
        // process lifetime; tokio will reap it on shutdown.
        std::mem::drop(crate::shutdown::install_sighup_handler(Arc::new(reloader)));
    } else {
        tracing::warn!(
            cert = %cfg.tls.cert_path.display(),
            key = %cfg.tls.key_path.display(),
            "cert-watch reloader could not load initial config; SIGHUP will be a no-op"
        );
    }

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

// Decision (M7 Sub-phase C, BRO-938 follow-up #1, Option B):
//
// `KmsProvider::Dev` is now **fail-closed** unless `dev_signer_enabled`
// is also `true`. The Sub-phase B default flowed `Dev` through silently
// even in production deployments, defeating the §5.4 KMS isolation
// invariant. We mirror the lifed pattern ("production startup with
// `--no-kms` is rejected unless `--dev-mode` is also set"): operators
// must set BOTH `auth.kms_provider = "dev"` AND
// `auth.dev_signer_enabled = true` to opt into the in-process
// keystore. Either alone is rejected. Production builds use
// Vault/AWS/GCP — the `kms-vault` feature ships as the default in
// `Cargo.toml`. Option A (flip `KmsProvider::default()` to `Vault`)
// was considered and rejected: a config with no `[auth]` block would
// silently land on Vault and crash at startup with a non-obvious
// "vault not configured" error. Option B fails fast at signer-build
// time with a clear invariant message.

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

    // Sub-phase D (D7): build the JWKS cache once and thread it
    // explicitly through `AuthLayer::with_jwks`. The legacy
    // process-global `dev_signer::install_tier1_verifier` shim is
    // kept in place for tests that still reach the deprecated entry
    // point; production code stops touching that global.
    let jwks = build_jwks_cache(&cfg.auth)?;

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
    // Sub-phase D (D1): build the rate limiter from config and
    // attach it to the auth layer. Production deploys ALWAYS run
    // with the limiter wired; tests can opt-out by constructing
    // `AuthLayer::with_jwks(...)` directly.
    let rate_limiter = TokenBucketLimiter::from_config(&cfg.rate_limit);
    let auth_layer = AuthLayer::with_jwks(
        minter,
        cfg.auth.dev_signer_enabled,
        Arc::clone(&upstream_path),
        Arc::clone(&jwks),
    )
    .with_rate_limiter(rate_limiter.clone());

    // Sub-phase D (D3) + Sub-phase E sweep (item #14): construct the
    // cert reloader from the same cert + key paths the bind step
    // used. The reloader holds an ArcSwap<ServerConfig> so the
    // listener path can swap configs atomically without disrupting
    // in-flight TLS connections.
    //
    // Sub-phase E sweep (item #14) closes the previously-deferred
    // hot-swap into `serve_connections` — see the AcceptorSource
    // construction below where the reloader is wired into the accept
    // loop. The admin-plane `CertReload` RPC routes through the
    // reloader so operators retain the ad-hoc reload handle; the
    // SIGHUP handler installed at startup also bumps the reload
    // counter for dashboards.
    let cert_reloader = CertReloader::load(&cfg.tls.cert_path, &cfg.tls.key_path)
        .ok()
        .map(Arc::new);
    if cert_reloader.is_none() {
        tracing::warn!(
            cert = %cfg.tls.cert_path.display(),
            key = %cfg.tls.key_path.display(),
            "cert-watch reloader could not load initial config; \
             admin-plane CertReload will return a no-op success and \
             the listener will hold the bind() acceptor for the daemon's life"
        );
    }
    let cert_hook = match cert_reloader.as_ref() {
        Some(rel) => {
            let rel = Arc::clone(rel);
            CertReloadHook::new(move |_force| match rel.reload() {
                Ok(n) => crate::admin::service::CertReloadOutcome::reloaded(n as u32),
                Err(e) => crate::admin::service::CertReloadOutcome::rejected(e.to_string()),
            })
        }
        None => CertReloadHook::noop(),
    };

    // Sub-phase D (D2): admin plane. Bind the admin UDS in parallel
    // with the public plane. The admin server runs on its own
    // tonic::transport::Server::builder() driven by the AdminAcceptor
    // (which yields AdminConn carrying SO_PEERCRED). Tests that don't
    // need admin can ignore this — the `serve_with_listener_*` paths
    // always start it (mirroring lifed) so the admin RPCs land in
    // every test rig.
    let blocklist = Blocklist::new();
    // Sub-phase E sweep (item #13): admin metrics handle threaded into
    // the policy so group-lookup fail-closed denials advance the
    // `gateway.admin.rejected_total{reason="group_lookup"}` counter.
    let admin_metrics = AdminMetrics::new();
    let admin_policy = build_admin_policy(&cfg.admin_plane, admin_metrics.clone());
    let admin_service = GatewayAdminService::new(
        Arc::new(admin_policy),
        blocklist.clone(),
        rate_limiter.clone(),
        Arc::clone(&jwks),
        cert_hook,
    );
    let admin_acceptor = admin_listener::bind(&cfg.admin_plane).await?;
    let (admin_shutdown_tx, admin_shutdown_rx) = oneshot::channel::<()>();
    let admin_handle = tokio::spawn(async move {
        let svc = admin_pb::gateway_admin_server::GatewayAdminServer::new(admin_service);
        let res = tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming_shutdown(admin_acceptor, async {
                let _ = admin_shutdown_rx.await;
            })
            .await;
        if let Err(err) = res {
            tracing::warn!(error = %err, "admin plane server exited with error");
        }
    });
    tracing::info!(
        admin_socket = %cfg.admin_plane.unix_socket.display(),
        "lifegw admin plane bound"
    );

    let agent = AgentForwarder::new(upstream_channel.clone());
    let events = EventsForwarder::new(upstream_channel.clone());
    let wallet = WalletForwarder::new(upstream_channel.clone());
    let identity = IdentityForwarder::new(upstream_channel.clone());

    // Sub-phase C (BRO-938 C1): wire the WebSocket dispatcher Layer
    // BELOW AuthLayer so the Tier-1 verify + Tier-2 mint + scope
    // check still run before the upgrade response is sent. The
    // WsLayer holds an Arc'd `AgentClient<Channel>` for the bidi
    // pump's upstream calls (`Agent.StreamSession`,
    // `Agent.SendMessage`, `Agent.{Approve,Cancel}Dispatch`). The
    // Layer falls through to the tonic stack for non-WS paths.
    let ws_upstream = pb::agent_client::AgentClient::new(upstream_channel.clone());
    let ws_layer = WsLayer::new(ws_upstream);

    // Sub-phase C refactor (BRO-938): we need WS upgrade support, but
    // tonic 0.14's `Server::serve_with_incoming_shutdown` does not
    // wire `hyper::upgrade::on(req)` through to the underlying hyper
    // connection. We therefore build the tonic `Routes` (the same
    // axum-router-backed dispatcher tonic uses internally), wrap it
    // in our tower stack (auth → ws-dispatch → grpc-web → routes),
    // and drive each TLS-accepted connection through
    // `hyper_util::server::conn::auto::Builder::serve_connection_with_upgrades`.
    // That preserves H1+H2 auto-detection for native gRPC + Connect,
    // and enables WS upgrades for `/v1/agent/stream`.
    let mut routes_builder = Routes::builder();
    routes_builder.add_service(pb::agent_server::AgentServer::new(agent));
    routes_builder.add_service(pb::events_server::EventsServer::new(events));
    routes_builder.add_service(pb::wallet_server::WalletServer::new(wallet));
    routes_builder.add_service(pb::identity_server::IdentityServer::new(identity));
    let routes = routes_builder.routes().prepare();

    let service = ServiceBuilder::new()
        .layer(auth_layer)
        .layer(ws_layer)
        .layer(GrpcWebLayer::new())
        .service(routes);

    let TlsBind {
        acceptor,
        listener,
        local_addr,
    } = bind;

    tracing::info!(addr = %local_addr, "lifegw listening");

    // Sub-phase E sweep (item #14): if the cert reloader successfully
    // loaded above, route accepts through it so cert rotations reach
    // the per-connection handshake. The reloader's bg watcher already
    // updates the underlying ArcSwap<ServerConfig>; wiring the source
    // here ensures the listener consumes the swap. When the reloader
    // failed to load (e.g. tests with disposable certs), fall back to
    // the bind result's static acceptor.
    let acceptor_source = match cert_reloader.as_ref() {
        Some(r) => AcceptorSource::Reloader((**r).clone()),
        None => AcceptorSource::Static(acceptor),
    };

    // Run the public plane until shutdown. When it exits (graceful
    // drain or accept error), tear down the admin plane too so we
    // don't leak the bound socket.
    let result = serve_connections(listener, acceptor_source, service, shutdown_rx).await;
    let _ = admin_shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), admin_handle).await;
    result
}

fn build_admin_policy(cfg: &AdminPlaneConfig, metrics: AdminMetrics) -> AdminPolicy {
    // Resolve the admin GID — fall back to permissive mode if the
    // group isn't configured OR the lookup fails (matches the lifed
    // pattern; the systemd unit enforces FS-level access in the
    // group-unconfigured case). Sub-phase E sweep (item #13): the
    // policy is built with a metric handle so group-lookup
    // fail-closed denials advance the
    // `gateway.admin.rejected_total{reason="group_lookup"}` counter.
    match cfg.unix_socket_group.as_deref() {
        Some(name) => match crate::admin::peercred::group_gid(name) {
            Ok(Some(gid)) => AdminPolicy::strict(gid).with_metrics(metrics),
            _ => {
                tracing::warn!(
                    group = name,
                    "unix_socket_group not found in /etc/group; admin plane is in permissive mode"
                );
                AdminPolicy::permissive().with_metrics(metrics)
            }
        },
        None => AdminPolicy::permissive().with_metrics(metrics),
    }
}

/// Box up an error into the dyn StdError shape hyper expects. Used
/// inside the per-connection `service_fn` to keep the closure body
/// concise and pin the lifetime to `'static`.
#[inline]
fn boxed_err<E>(err: E) -> Box<dyn std::error::Error + Send + Sync>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    err.into()
}

/// Sub-phase E sweep (item #14): hot-swappable source of TLS acceptors.
///
/// The accept loop calls [`AcceptorSource::current`] on every new
/// connection so cert rotations reach the per-connection handshake.
/// In production this wraps a [`CertReloader`] which holds an
/// `ArcSwap<rustls::ServerConfig>`; under test (or when the reloader
/// failed to load initial config) it wraps a single static
/// `TlsAcceptor` and behaves like Sub-phase D's pre-rotate path.
#[derive(Clone)]
enum AcceptorSource {
    Static(tokio_rustls::TlsAcceptor),
    Reloader(CertReloader),
}

impl AcceptorSource {
    fn current(&self) -> tokio_rustls::TlsAcceptor {
        match self {
            AcceptorSource::Static(a) => a.clone(),
            AcceptorSource::Reloader(r) => r.acceptor(),
        }
    }
}

/// 500-shaped response for inner-service failures. Caller logs the
/// underlying error before invoking this. Used by the per-connection
/// `service_fn` to keep hyper's service Infallible.
fn internal_error_response() -> http::Response<tonic::body::Body> {
    let mut resp = http::Response::new(tonic::body::Body::empty());
    *resp.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
    resp
}

/// Drive each accepted connection through hyper-util's auto Builder
/// with `serve_connection_with_upgrades` so WS upgrades reach
/// [`crate::services::ws::handle_upgrade`]. The `service` is cloned
/// per-connection (cheap — it's a tower stack of `Arc`/`Channel`
/// handles).
///
/// Sub-phase E sweep (item #14): the `acceptor` parameter is now an
/// `AcceptorSource` rather than a single static `TlsAcceptor`. Each
/// new accept reads a fresh `TlsAcceptor` from the source so cert
/// rotations reach the listener accept loop, not just the cert
/// reloader's `current()` accessor. Pre-existing TLS connections keep
/// the config they handshook with via rustls's `Arc<ServerConfig>`
/// semantics — this is non-disruptive to in-flight traffic.
///
/// The body-type bridge: hyper feeds the inbound service
/// `Request<hyper::body::Incoming>`. Tonic's auth + ws + grpc-web
/// stack expects `Request<tonic::body::Body>`. The per-connection
/// `service_fn` maps `Incoming → tonic::body::Body` via
/// `Body::new(incoming)` before calling the tower stack.
async fn serve_connections<S>(
    listener: tokio::net::TcpListener,
    acceptor: AcceptorSource,
    service: S,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> LifegwResult<()>
where
    S: tower::Service<
            http::Request<tonic::body::Body>,
            Response = http::Response<tonic::body::Body>,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send + 'static,
{
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                tracing::info!("lifegw shutdown signal received");
                return Ok(());
            }
            accept = listener.accept() => {
                match accept {
                    Ok((sock, _peer)) => {
                        // Sub-phase E sweep (item #14): refresh the
                        // TlsAcceptor on each new accept so cert
                        // rotations reach the per-connection handshake.
                        let acceptor = acceptor.current();
                        let service = service.clone();
                        tokio::spawn(async move {
                            let tls = match acceptor.accept(sock).await {
                                Ok(t) => t,
                                Err(e) => {
                                    tracing::debug!(error = %e, "tls handshake failed");
                                    return;
                                }
                            };
                            // Per-connection hyper service. Maps
                            // hyper::body::Incoming → tonic::body::Body
                            // before delegating to the tower stack.
                            // Errors from the inner service are
                            // surfaced as `500 Internal Server Error`
                            // responses (logged) rather than
                            // propagated to hyper — this keeps the
                            // service `Infallible` from hyper's
                            // perspective and dodges the HRTB
                            // constraint on `Box<dyn StdError + 'static>`
                            // that arises when service_fn captures a
                            // generic-error tower stack.
                            let svc = hyper::service::service_fn(
                                move |req: http::Request<hyper::body::Incoming>| {
                                    let mut s = service.clone();
                                    let req = req.map(tonic::body::Body::new);
                                    async move {
                                        if let Err(err) = ServiceExt::ready(&mut s).await {
                                            tracing::warn!(
                                                error = %boxed_err(err),
                                                "inner service not ready"
                                            );
                                            return Ok::<_, std::convert::Infallible>(
                                                internal_error_response(),
                                            );
                                        }
                                        match s.call(req).await {
                                            Ok(resp) => Ok(resp),
                                            Err(err) => {
                                                tracing::warn!(
                                                    error = %boxed_err(err),
                                                    "inner service error"
                                                );
                                                Ok(internal_error_response())
                                            }
                                        }
                                    }
                                },
                            );
                            let builder = HyperConnBuilder::new(TokioExecutor::new());
                            if let Err(err) = builder
                                .serve_connection_with_upgrades(TokioIo::new(tls), svc)
                                .await
                            {
                                tracing::debug!(error = %err, "connection terminated");
                            }
                        });
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "accept failed");
                        // Brief backoff so a flapping listener doesn't
                        // pin the CPU.
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        }
    }
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
///
/// **Sub-phase C hardening (Option B, BRO-938 follow-up #1)**:
/// `KmsProvider::Dev` is gated behind `dev_signer_enabled = true`. The
/// only way to land on the in-process `StaticKeystore` signer is to
/// explicitly opt in via BOTH config fields. This mirrors the lifed
/// rule ("production startup with `--no-kms` is rejected unless
/// `--dev-mode` is also set") and prevents a silent
/// `KmsProvider::default()` → `Dev` foot-gun in production deployments.
pub(crate) fn build_signer(cfg: &AuthConfig) -> LifegwResult<Arc<dyn KmsSigner>> {
    match cfg.kms_provider {
        KmsProvider::Dev => {
            if !cfg.dev_signer_enabled {
                return Err(LifegwError::Config(
                    "auth.kms_provider = dev requires auth.dev_signer_enabled = true \
                     (Sub-phase C hardening — production deploys must use a real KMS provider)"
                        .to_string(),
                ));
            }
            Ok(Arc::new(StaticKeystore::generate_dev()?))
        }
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
        KmsProvider::Aws => Err(LifegwError::Config(
            "kms-aws provider configured but body deferred to Sub-phase E".to_string(),
        )),
        #[cfg(not(feature = "kms-aws"))]
        KmsProvider::Aws => Err(LifegwError::Config(
            "auth.kms_provider = aws but lifegw built without `kms-aws` feature".to_string(),
        )),
        #[cfg(feature = "kms-gcp")]
        KmsProvider::Gcp => Err(LifegwError::Config(
            "kms-gcp provider configured but body deferred to Sub-phase E".to_string(),
        )),
        #[cfg(not(feature = "kms-gcp"))]
        KmsProvider::Gcp => Err(LifegwError::Config(
            "auth.kms_provider = gcp but lifegw built without `kms-gcp` feature".to_string(),
        )),
    }
}

/// Build a Tier-1 JWKS cache from `cfg.auth`. Sub-phase D (D7) stops
/// installing this into the deprecated process-global; instead the
/// returned `Arc<JwksCache>` is threaded into `AuthLayer::with_jwks`
/// so each `AuthService<S>` instance owns its own handle.
fn build_jwks_cache(cfg: &AuthConfig) -> LifegwResult<Arc<JwksCache>> {
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
    Ok(cache)
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
    fn build_signer_dev_requires_dev_signer_enabled() {
        // Sub-phase C hardening (BRO-938 follow-up #1, Option B):
        // KmsProvider::Dev with dev_signer_enabled = false is rejected.
        let cfg = AuthConfig {
            kms_provider: KmsProvider::Dev,
            dev_signer_enabled: false,
            ..AuthConfig::default()
        };
        match build_signer(&cfg) {
            Ok(_) => panic!("must reject Dev kms_provider without dev_signer_enabled"),
            Err(LifegwError::Config(m)) => {
                assert!(
                    m.contains("dev_signer_enabled"),
                    "rejection mentions dev_signer_enabled: {m}"
                );
            }
            Err(other) => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn build_signer_dev_accepted_when_dev_signer_enabled() {
        // Sub-phase C hardening: Dev provider IS allowed when the
        // operator explicitly opts in via dev_signer_enabled = true.
        let cfg = AuthConfig {
            kms_provider: KmsProvider::Dev,
            dev_signer_enabled: true,
            ..AuthConfig::default()
        };
        let signer = build_signer(&cfg).expect("Dev + dev_signer_enabled is allowed");
        assert!(!signer.active_kid().is_empty());
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
