//! Tower middleware that runs Tier-1 verify (in) → Tier-2 mint (out).
//!
//! Per Spec C₃ §5.1, every public-plane request:
//!
//! 1. Reads the `authorization` header → strips `Bearer `.
//! 2. Validates the bearer via the dev signer (Sub-phase A) or the real
//!    Vercel JWKS verifier (Sub-phase B).
//! 3. **Sub-phase C (BRO-938 follow-up #3)**: enforces
//!    `(Tier-1 scopes) ∩ (route required scope) ≠ ∅`
//!    *before* Tier-2 is minted (Spec C₃ §5.4). Empty intersection →
//!    `Status::permission_denied("scope insufficient")`. Unknown routes
//!    → `Status::not_found` (don't leak route existence to forged-scope
//!    probes).
//! 4. Mints a Tier-2 capability JWS via the in-process keystore (A) or KMS
//!    (E). Audience `lifed`, issuer `lifegw`, lifetime ≤ 15 min.
//! 5. Replaces the inbound `authorization` header with the Tier-2 JWS.
//! 6. Forwards to the proxy service.
//!
//! Health endpoints (`/healthz`, `/readyz`, `/version`, `/metrics`) bypass
//! this layer per Spec C₃ §3.5 LOCKED L4-D7. Sub-phase A handles `/healthz`
//! inline here so the gateway can answer health checks without standing up a
//! second listener.

use std::path::PathBuf;
use std::sync::Arc;
use std::task::{Context, Poll};

use http::Request;
use tonic::body::Body;
use tower::{Layer, Service};

use crate::auth::dev_signer;
use crate::auth::scope::{self, ScopeError};
use crate::auth::tier2::Tier2Minter;
use crate::services::health;

/// Tower Layer wrapping a service with Tier-1 verify + Tier-2 mint and
/// a `/healthz` bypass path.
///
/// Future fields (rate-limiter handle in Sub-phase D, scope-intersection
/// table in Sub-phase B's follow-up) are added without breaking
/// downstream consumers because the type is `#[non_exhaustive]`.
#[derive(Clone)]
#[non_exhaustive]
pub struct AuthLayer {
    minter: Arc<Tier2Minter>,
    dev_signer_enabled: bool,
    upstream_path: Arc<PathBuf>,
}

impl AuthLayer {
    /// Construct a new `AuthLayer`.
    ///
    /// # Parameters
    /// - `minter` — Tier-2 capability-token mint helper. Owns the KMS
    ///   signer the gateway uses to issue capability tokens to lifed.
    /// - `dev_signer_enabled` — when `true`, accept the magic
    ///   `Bearer dev-token-for-{user_id}` shortcut alongside real JWS
    ///   tokens. MUST be `false` in production deployments.
    /// - `upstream_path` — UDS path to lifed. The `/healthz` bypass
    ///   probes this socket to confirm upstream readiness.
    pub fn new(
        minter: Arc<Tier2Minter>,
        dev_signer_enabled: bool,
        upstream_path: Arc<PathBuf>,
    ) -> Self {
        Self {
            minter,
            dev_signer_enabled,
            upstream_path,
        }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            minter: Arc::clone(&self.minter),
            dev_signer_enabled: self.dev_signer_enabled,
            upstream_path: Arc::clone(&self.upstream_path),
        }
    }
}

/// Inner tower `Service` produced by [`AuthLayer::layer`]. Holds a
/// reference to the wrapped upstream service and re-runs the
/// Tier-1 verify → Tier-2 mint → forward sequence on every inbound
/// request.
///
/// The struct is `#[non_exhaustive]` so adding fields in later
/// sub-phases (e.g. a per-request rate-limiter handle in D, a
/// scope-intersection table after B-follow-up) does not break
/// consumers that build the type via [`AuthLayer::layer`] (which is
/// the only sanctioned constructor).
#[derive(Clone)]
#[non_exhaustive]
pub struct AuthService<S> {
    inner: S,
    minter: Arc<Tier2Minter>,
    dev_signer_enabled: bool,
    upstream_path: Arc<PathBuf>,
}

impl<S> Service<Request<Body>> for AuthService<S>
where
    S: Service<Request<Body>, Response = http::Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        let minter = Arc::clone(&self.minter);
        let dev_signer_enabled = self.dev_signer_enabled;
        let upstream_path = Arc::clone(&self.upstream_path);

        Box::pin(async move {
            // Spec C₃ §3.5 LOCKED L4-D7: health endpoints bypass auth.
            if req.uri().path() == "/healthz" {
                return Ok(health::handle(upstream_path).await);
            }

            let bearer = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|h| h.strip_prefix("Bearer "))
                .map(|t| t.to_string());

            // `dev_signer_enabled` is informational here — both code
            // paths route through `dev_signer::verify`, which delegates
            // to the global `JwksCache`. Whether the cache accepts the
            // dev shortcut or runs real ES256 / RS256 is decided by the
            // cache type bootstrap installed (see
            // `bootstrap::install_tier1_verifier`). We capture the flag
            // for logging only.
            let _ = dev_signer_enabled;
            let tier1 = match bearer {
                Some(tok) => match dev_signer::verify(&tok) {
                    Ok(c) => c,
                    Err(_) => return Ok(unauth_response("invalid Tier-1 bearer")),
                },
                None => return Ok(unauth_response("missing Tier-1 bearer token")),
            };

            // Sub-phase C (BRO-938 follow-up #3): scope intersection
            // check BEFORE the Tier-2 mint pays its CPU cost. Empty
            // intersection means a forbidden route never gets a usable
            // capability token — the attacker stops at the gateway
            // boundary instead of churning JWS work.
            let path = req.uri().path().to_string();
            match scope::enforce(&path, &tier1) {
                Ok(()) => {}
                Err(ScopeError::Insufficient {
                    required,
                    available,
                    ..
                }) => {
                    tracing::debug!(
                        route = %path,
                        required = %required,
                        available = ?available,
                        user = %tier1.user_id,
                        "scope insufficient — denying before Tier-2 mint"
                    );
                    return Ok(permission_denied_response("scope insufficient"));
                }
                Err(ScopeError::UnknownRoute(p)) => {
                    tracing::debug!(route = %p, "unknown route — returning Status::not_found");
                    return Ok(not_found_response("unknown route"));
                }
            }

            let tier2 = match minter.mint(&tier1) {
                Ok(t) => t,
                Err(e) => return Ok(internal_response(&format!("tier-2 mint: {e}"))),
            };

            // Replace the inbound bearer with the Tier-2 JWS so the upstream
            // lifed verifier receives a token signed by lifegw.
            let new_value = match http::HeaderValue::from_str(&format!("Bearer {tier2}")) {
                Ok(v) => v,
                Err(e) => return Ok(internal_response(&format!("tier-2 header: {e}"))),
            };
            req.headers_mut().insert("authorization", new_value);

            inner.call(req).await
        })
    }
}

/// Build a `Status::unauthenticated`-shaped HTTP response that tonic clients
/// surface as a `tonic::Status` of code `Unauthenticated`. Same trick lifed
/// uses (`crates/life-runtime/lifed/src/auth/middleware.rs::unauth_response`).
fn unauth_response(msg: &str) -> http::Response<Body> {
    let status = tonic::Status::unauthenticated(msg.to_string());
    grpc_status_response(status)
}

fn internal_response(msg: &str) -> http::Response<Body> {
    let status = tonic::Status::internal(msg.to_string());
    grpc_status_response(status)
}

/// Build a `Status::permission_denied`-shaped HTTP response. Used by
/// the scope intersection enforcement: an authenticated bearer that
/// fails the route's scope check stops here.
fn permission_denied_response(msg: &str) -> http::Response<Body> {
    let status = tonic::Status::permission_denied(msg.to_string());
    grpc_status_response(status)
}

/// Build a `Status::not_found`-shaped HTTP response. Used when the
/// inbound path doesn't match any known route — keeps scope-forgery
/// probes from learning which routes exist.
fn not_found_response(msg: &str) -> http::Response<Body> {
    let status = tonic::Status::not_found(msg.to_string());
    grpc_status_response(status)
}

fn grpc_status_response(status: tonic::Status) -> http::Response<Body> {
    let mut resp = http::Response::new(Body::empty());
    *resp.status_mut() = http::StatusCode::OK;
    let headers = resp.headers_mut();
    headers.insert(
        "content-type",
        http::HeaderValue::from_static("application/grpc"),
    );
    headers.insert(
        "grpc-status",
        http::HeaderValue::from_str(&(status.code() as i32).to_string())
            .unwrap_or_else(|_| http::HeaderValue::from_static("13")),
    );
    if let Ok(v) = http::HeaderValue::from_str(status.message()) {
        headers.insert("grpc-message", v);
    }
    resp
}
