//! `life.admin.gw.v1.GatewayAdmin` service body. Sub-phase D (D2).
//!
//! Each handler:
//! 1. Pulls the connection's `PeerCred` out of the request extensions
//!    (placed there by `admin::listener::AdminAcceptor`).
//! 2. Authorises via [`AdminPolicy::check`].
//! 3. Reads or mutates the in-memory state and returns. No business
//!    logic outside the gateway boundary.
//!
//! Admin handlers may exceed the public-plane ≤20-LOC budget — list
//! and dump ops naturally take more lines — but they NEVER hold a
//! lock across an `await`.

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use tonic::{Request, Response, Status};

use life_runtime_proto::life::admin::gw::v1 as adm;

use crate::admin::blocklist::Blocklist;
use crate::admin::listener::AdminConnInfo;
use crate::admin::peercred::PeerCred;
use crate::admin::policy::{AdminOp, AdminPolicy};
use crate::auth::jwks::JwksCache;
use crate::services::rate_limit::{RateLimitOverride, TokenBucketLimiter};

/// Hook that fires when the admin plane requests a cert reload. The
/// hook owns the cert reloader so the admin service doesn't need to
/// reach into `listener.rs` directly.
#[derive(Clone)]
pub struct CertReloadHook {
    inner: Arc<dyn Fn(bool) -> CertReloadOutcome + Send + Sync>,
}

impl CertReloadHook {
    pub fn new(f: impl Fn(bool) -> CertReloadOutcome + Send + Sync + 'static) -> Self {
        Self { inner: Arc::new(f) }
    }

    /// Test/no-op hook that always reports success with one cert.
    pub fn noop() -> Self {
        Self::new(|_| CertReloadOutcome::reloaded(1))
    }

    /// Test/no-op hook that always reports failure with the given
    /// reason. Used to verify the proto carries failure metadata.
    pub fn fail(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self::new(move |_| CertReloadOutcome::rejected(reason.clone()))
    }

    fn invoke(&self, force: bool) -> CertReloadOutcome {
        (self.inner)(force)
    }
}

/// Outcome of a cert-reload attempt.
#[derive(Debug, Clone)]
pub struct CertReloadOutcome {
    pub reloaded: bool,
    pub cert_count: u32,
    pub reason: String,
}

impl CertReloadOutcome {
    pub fn reloaded(cert_count: u32) -> Self {
        Self {
            reloaded: true,
            cert_count,
            reason: String::new(),
        }
    }

    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            reloaded: false,
            cert_count: 0,
            reason: reason.into(),
        }
    }
}

/// Concrete `GatewayAdmin` service. Holds handles to the four pieces
/// of in-memory state the admin RPCs touch: the policy table, the
/// blocklist, the rate-limit override registry (delegated to the
/// `TokenBucketLimiter`), and the JWKS cache.
pub struct GatewayAdminService {
    pub policy: Arc<AdminPolicy>,
    pub blocklist: Blocklist,
    pub rate_limiter: TokenBucketLimiter,
    pub jwks: Arc<JwksCache>,
    pub cert_reload: CertReloadHook,
    pub version: &'static str,
}

impl GatewayAdminService {
    pub fn new(
        policy: Arc<AdminPolicy>,
        blocklist: Blocklist,
        rate_limiter: TokenBucketLimiter,
        jwks: Arc<JwksCache>,
        cert_reload: CertReloadHook,
    ) -> Self {
        Self {
            policy,
            blocklist,
            rate_limiter,
            jwks,
            cert_reload,
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn cred<T>(req: &Request<T>) -> Result<PeerCred, Status> {
        req.extensions()
            .get::<AdminConnInfo>()
            .map(|c| c.cred)
            .ok_or_else(|| Status::internal("admin connection lacks PeerCred"))
    }
}

#[tonic::async_trait]
impl adm::gateway_admin_server::GatewayAdmin for GatewayAdminService {
    async fn health_check(
        &self,
        req: Request<adm::HealthReq>,
    ) -> Result<Response<adm::HealthResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::HealthCheck)?;
        Ok(Response::new(adm::HealthResp {
            ok: true,
            version: self.version.to_string(),
            // Best-effort — the admin plane doesn't probe the lifed
            // UDS itself; that signal is on `/healthz` for the public
            // plane. We report `true` here as a placeholder; future
            // sub-phases can wire a heartbeat probe.
            lifed_reachable: true,
            active_ws_connections: 0,
            jwks_cached_kids: self.jwks.active_key_count() as u32,
        }))
    }

    async fn cert_reload(
        &self,
        req: Request<adm::CertReloadReq>,
    ) -> Result<Response<adm::CertReloadResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::CertReload)?;
        let force = req.get_ref().force;
        let outcome = self.cert_reload.invoke(force);
        Ok(Response::new(adm::CertReloadResp {
            reloaded: outcome.reloaded,
            cert_count: outcome.cert_count,
            reason: outcome.reason,
        }))
    }

    async fn jwks_dump(
        &self,
        req: Request<adm::JwksDumpReq>,
    ) -> Result<Response<adm::JwksDumpResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::JwksDump)?;
        // Sub-phase E sweep (item #11): JwksCache now exposes a real
        // `dump()` method that returns kid + alg + crv + retired
        // metadata for every entry. Operators can answer "is the cache
        // holding the kid the gateway just minted against?" without
        // shelling onto the host. Per the dump contract no key
        // material (PEM, x/y) leaves the cache.
        let keys = self
            .jwks
            .dump()
            .into_iter()
            .map(|d| adm::JwksKey {
                kid: d.kid,
                alg: d.alg,
                crv: d.crv,
                retired: d.retired,
                retired_at_epoch_millis: d.retired_at_epoch_millis,
            })
            .collect();
        Ok(Response::new(adm::JwksDumpResp { keys }))
    }

    async fn blocklist_add(
        &self,
        req: Request<adm::BlocklistAddReq>,
    ) -> Result<Response<adm::AdminAck>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::BlocklistAdd)?;
        let body = req.into_inner();
        if body.subject.is_empty() {
            return Err(Status::invalid_argument("missing subject"));
        }
        self.blocklist.add(body.subject, body.reason);
        Ok(Response::new(adm::AdminAck {}))
    }

    async fn blocklist_remove(
        &self,
        req: Request<adm::BlocklistRemoveReq>,
    ) -> Result<Response<adm::AdminAck>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::BlocklistRemove)?;
        let body = req.into_inner();
        if body.subject.is_empty() {
            return Err(Status::invalid_argument("missing subject"));
        }
        self.blocklist.remove(&body.subject);
        Ok(Response::new(adm::AdminAck {}))
    }

    async fn blocklist_list(
        &self,
        req: Request<adm::BlocklistListReq>,
    ) -> Result<Response<adm::BlocklistListResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::BlocklistList)?;
        // Sub-phase E sweep (item #8): direct u64 epoch-millis on the
        // wire — no `prost_types::Timestamp` round-trip on the hot
        // path.
        let entries = self
            .blocklist
            .list()
            .into_iter()
            .map(|e| adm::BlocklistEntry {
                subject: e.subject,
                reason: e.reason,
                added_at_epoch_millis: e
                    .added_at
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .and_then(|d| u64::try_from(d.as_millis()).ok())
                    .unwrap_or(0),
            })
            .collect();
        Ok(Response::new(adm::BlocklistListResp { entries }))
    }

    async fn rate_limit_override(
        &self,
        req: Request<adm::RateLimitOverrideReq>,
    ) -> Result<Response<adm::AdminAck>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::RateLimitOverride)?;
        let body = req.into_inner();
        if body.user_id.is_empty() {
            return Err(Status::invalid_argument("missing user_id"));
        }
        // Strip optional `user:` prefix so admin clients can use
        // either form.
        let user = body.user_id.strip_prefix("user:").unwrap_or(&body.user_id);
        self.rate_limiter.set_user_override(
            user,
            RateLimitOverride {
                capacity: body.capacity,
                refill_per_sec: body.refill_per_sec,
            },
        );
        Ok(Response::new(adm::AdminAck {}))
    }
}
