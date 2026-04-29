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
use std::time::SystemTime;

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
        // The cache exposes total + active counts but not the per-key
        // metadata directly. Sub-phase D ships an aggregate-only view
        // (kid + alg unavailable from the public API today; see the
        // follow-up note in CLAUDE.md). The proto field set is
        // forward-compatible — when the cache adds a `dump()` method
        // in Sub-phase E we can populate the per-key entries.
        let mut keys = Vec::new();
        for _ in 0..self.jwks.active_key_count() {
            keys.push(adm::JwksKey {
                kid: "<active>".to_string(),
                alg: "ES256".to_string(),
                retired: false,
                retired_at: None,
            });
        }
        let total = self.jwks.total_key_count();
        let active = self.jwks.active_key_count();
        for _ in 0..total.saturating_sub(active) {
            keys.push(adm::JwksKey {
                kid: "<retired>".to_string(),
                alg: "ES256".to_string(),
                retired: true,
                retired_at: None,
            });
        }
        Ok(Response::new(adm::JwksDumpResp { keys }))
    }

    async fn blocklist_add(
        &self,
        req: Request<adm::BlocklistAddReq>,
    ) -> Result<Response<adm::BlocklistEmpty>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::BlocklistAdd)?;
        let body = req.into_inner();
        if body.subject.is_empty() {
            return Err(Status::invalid_argument("missing subject"));
        }
        self.blocklist.add(body.subject, body.reason);
        Ok(Response::new(adm::BlocklistEmpty {}))
    }

    async fn blocklist_remove(
        &self,
        req: Request<adm::BlocklistRemoveReq>,
    ) -> Result<Response<adm::BlocklistEmpty>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::BlocklistRemove)?;
        let body = req.into_inner();
        if body.subject.is_empty() {
            return Err(Status::invalid_argument("missing subject"));
        }
        self.blocklist.remove(&body.subject);
        Ok(Response::new(adm::BlocklistEmpty {}))
    }

    async fn blocklist_list(
        &self,
        req: Request<adm::BlocklistListReq>,
    ) -> Result<Response<adm::BlocklistListResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::BlocklistList)?;
        let entries = self
            .blocklist
            .list()
            .into_iter()
            .map(|e| adm::BlocklistEntry {
                subject: e.subject,
                reason: e.reason,
                added_at: Some(prost_types::Timestamp::from(
                    e.added_at
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| {
                            let secs = d.as_secs() as i64;
                            let nanos = d.subsec_nanos() as i32;
                            (secs, nanos)
                        })
                        .map(|(secs, nanos)| {
                            std::time::SystemTime::UNIX_EPOCH
                                + std::time::Duration::new(secs as u64, nanos as u32)
                        })
                        .unwrap_or(SystemTime::UNIX_EPOCH),
                )),
            })
            .collect();
        Ok(Response::new(adm::BlocklistListResp { entries }))
    }

    async fn rate_limit_override(
        &self,
        req: Request<adm::RateLimitOverrideReq>,
    ) -> Result<Response<adm::BlocklistEmpty>, Status> {
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
        Ok(Response::new(adm::BlocklistEmpty {}))
    }
}
