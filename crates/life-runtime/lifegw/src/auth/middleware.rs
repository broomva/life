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
use crate::auth::jwks::JwksCache;
use crate::auth::scope::{self, ScopeError};
use crate::auth::tier1::Tier1Claims;
use crate::auth::tier2::Tier2Minter;
use crate::error::LifegwResult;
use crate::services::health;
use crate::services::rate_limit::{RateLimitDecision, TokenBucketLimiter};

/// Tower Layer wrapping a service with Tier-1 verify + Tier-2 mint
/// + per-user/per-IP rate limit and a `/healthz` bypass path.
///
/// Future fields (additional Sub-phase D operational signals like
/// blocklist + cert-reload counters) are added without breaking
/// downstream consumers because the type is `#[non_exhaustive]`.
#[derive(Clone)]
#[non_exhaustive]
pub struct AuthLayer {
    minter: Arc<Tier2Minter>,
    dev_signer_enabled: bool,
    upstream_path: Arc<PathBuf>,
    /// Sub-phase D (D7): explicit per-`AuthService<S>` JWKS cache
    /// handle. When `None`, the legacy global `OnceLock<JwksCache>`
    /// path is used (deprecated; will be removed in Sub-phase E). The
    /// recommended constructor [`AuthLayer::with_jwks`] passes the
    /// cache here so per-test verifier swaps are possible without
    /// touching process-global state.
    jwks: Option<Arc<JwksCache>>,
    /// Sub-phase D (D1): token-bucket rate limiter. When `None`, the
    /// limiter is bypassed (used by tests that don't need to assert
    /// the limit path). Production startups always set this.
    rate_limiter: Option<TokenBucketLimiter>,
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
    ///
    /// **Sub-phase D (D7):** prefer [`AuthLayer::with_jwks`] which
    /// takes an explicit `Arc<JwksCache>`. The legacy `new`
    /// constructor falls back to the deprecated process-global JWKS
    /// installed via [`crate::auth::dev_signer::install_tier1_verifier`].
    pub fn new(
        minter: Arc<Tier2Minter>,
        dev_signer_enabled: bool,
        upstream_path: Arc<PathBuf>,
    ) -> Self {
        Self {
            minter,
            dev_signer_enabled,
            upstream_path,
            jwks: None,
            rate_limiter: None,
        }
    }

    /// Construct an `AuthLayer` with an explicit JWKS handle threaded
    /// through to every `AuthService<S>` instance the layer produces.
    ///
    /// This is the recommended constructor as of Sub-phase D (D7) —
    /// it sidesteps the deprecated process-global `OnceLock<JwksCache>`
    /// and lets multiple tests in a single binary swap verifiers via
    /// the explicit per-service handle.
    pub fn with_jwks(
        minter: Arc<Tier2Minter>,
        dev_signer_enabled: bool,
        upstream_path: Arc<PathBuf>,
        jwks: Arc<JwksCache>,
    ) -> Self {
        Self {
            minter,
            dev_signer_enabled,
            upstream_path,
            jwks: Some(jwks),
            rate_limiter: None,
        }
    }

    /// Sub-phase D (D1): attach a [`TokenBucketLimiter`] to the
    /// layer. The limiter is consulted post-auth (so we have
    /// `Tier1Claims.user_id`) and pre–Tier-2 mint (so the gateway
    /// drops over-budget traffic before paying the JWS-mint CPU
    /// cost).
    pub fn with_rate_limiter(mut self, limiter: TokenBucketLimiter) -> Self {
        self.rate_limiter = Some(limiter);
        self
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
            jwks: self.jwks.clone(),
            rate_limiter: self.rate_limiter.clone(),
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
    /// Sub-phase D (D7): explicit JWKS cache handle threaded from
    /// [`AuthLayer::with_jwks`]. When `None`, the deprecated process
    /// global is consulted as a fallback.
    jwks: Option<Arc<JwksCache>>,
    /// Sub-phase D (D1): token-bucket limiter handle. When `None`,
    /// the limiter is bypassed (test paths only).
    rate_limiter: Option<TokenBucketLimiter>,
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
        // Sub-phase D (D7): the verifier handle now lives on
        // `self` so we capture an `Arc<JwksCache>` (or `None`) into
        // the future before the mutable borrow of `self.inner.clone()`
        // is dropped.
        let jwks_handle = self.jwks.clone();
        // Sub-phase D (D1): rate-limiter handle.
        let rate_limiter = self.rate_limiter.clone();

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
            // paths route through the JWKS cache. Whether the cache
            // accepts the dev shortcut or runs real ES256/RS256 is
            // decided by the cache type. The flag is captured for
            // logging only.
            let _ = dev_signer_enabled;
            let tier1 = match bearer {
                Some(tok) => match verify_with_handle(jwks_handle.as_ref(), &tok) {
                    Ok(c) => c,
                    Err(_) => return Ok(unauth_response("invalid Tier-1 bearer")),
                },
                None => return Ok(unauth_response("missing Tier-1 bearer token")),
            };

            // Sub-phase D (D1): rate-limit check AFTER Tier-1 verify
            // (so we have a real `user_id` to key the bucket on) and
            // BEFORE the Tier-2 mint (so over-budget traffic doesn't
            // pay the JWS-mint CPU cost). Per the prompt's hard rule,
            // we surface `Status::resource_exhausted(...)` — that
            // tonic code maps cleanly to WS close 4001 via the
            // existing close-code mapper.
            if let Some(limiter) = rate_limiter.as_ref() {
                let peer_ip = peer_ip_from_request(&req)
                    .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
                let decision = limiter.check(&tier1.user_id, peer_ip);
                if decision.is_reject() {
                    tracing::debug!(
                        user = %tier1.user_id,
                        ip = %peer_ip,
                        reason = decision.reason(),
                        "rate limit rejected request"
                    );
                    return Ok(rate_limit_response(decision));
                }
            }

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

/// Verify a Tier-1 bearer through the per-service handle when set,
/// otherwise the deprecated process-global. Sub-phase D (D7).
fn verify_with_handle(jwks: Option<&Arc<JwksCache>>, bearer: &str) -> LifegwResult<Tier1Claims> {
    if let Some(cache) = jwks {
        return cache.verify(bearer);
    }
    match dev_signer::global_verifier() {
        Some(cache) => cache.verify(bearer),
        None => Err(crate::error::LifegwError::Auth(
            "tier-1 verifier not installed (use AuthLayer::with_jwks(...))".to_string(),
        )),
    }
}

/// Resolve the request's peer IP for the rate-limiter's per-IP
/// bucket. Sub-phase D (D1).
///
/// Order of precedence:
/// 1. `X-Forwarded-For` header (left-most non-empty value). Used when
///    lifegw sits behind another L7 proxy / Vercel edge. We trust the
///    header because the systemd unit binds publicly only on
///    operator-controlled fronts; if you deploy lifegw without a
///    trusted L7 in front, set `LIFEGW_DISABLE_XFF=1` or remove this
///    branch in your fork.
/// 2. `Forwarded` header (RFC 7239) — `for=<ip>` token.
/// 3. `TcpConnectInfo` from tonic's `Connected` extension — the
///    socket-level peer address.
/// 4. None — caller falls back to `0.0.0.0` so the rate limiter still
///    enforces a single-shared-bucket budget (defence in depth).
fn peer_ip_from_request<B>(req: &Request<B>) -> Option<std::net::IpAddr> {
    use tonic::transport::server::TcpConnectInfo;
    // X-Forwarded-For — pick the leftmost non-empty token.
    if let Some(hv) = req.headers().get("x-forwarded-for")
        && let Ok(s) = hv.to_str()
        && let Some(first) = s.split(',').map(str::trim).find(|x| !x.is_empty())
        && let Some(ip) = parse_ip_or_socket(first)
    {
        return Some(ip);
    }
    // Forwarded: for=<ip> (RFC 7239).
    if let Some(hv) = req.headers().get("forwarded")
        && let Ok(s) = hv.to_str()
    {
        for part in s.split(';') {
            for kv in part.split(',') {
                let trimmed = kv.trim();
                let rest = trimmed
                    .strip_prefix("for=")
                    .or_else(|| trimmed.strip_prefix("For="));
                if let Some(rest) = rest {
                    let raw = rest.trim_matches('"');
                    if let Some(ip) = parse_ip_or_socket(raw) {
                        return Some(ip);
                    }
                }
            }
        }
    }
    // TonicConnectInfo — placed by `LifegwTlsStream::connect_info` per
    // connection.
    req.extensions()
        .get::<TcpConnectInfo>()
        .and_then(|ci| ci.remote_addr.map(|s| s.ip()))
}

/// Parse an IP literal that may carry an optional port. Sub-phase E
/// sweep (item #10): admin-plane error reporting + XFF parsing both
/// previously assumed IPv6 addresses arrive bracket-stripped. Real
/// world flows (`Forwarded: for="[::1]:443"`, `X-Forwarded-For: [::1]:443`,
/// `tonic remote_addr` debug output) carry brackets + ports.
///
/// Order of attempts:
/// 1. `[::1]:port` form — try `SocketAddr::from_str` (handles bracketed
///    IPv6 with port).
/// 2. `1.2.3.4:port` form — also via `SocketAddr::from_str`.
/// 3. Bare `IpAddr` — strip optional brackets, try `IpAddr::from_str`.
/// 4. IPv4-with-port (`1.2.3.4:port`) is already handled by
///    `SocketAddr::from_str` in step 2; we keep step 3 separate so the
///    caller can pass a bare IPv6 (e.g. `::1`) without brackets.
pub(crate) fn parse_ip_or_socket(input: &str) -> Option<std::net::IpAddr> {
    use std::net::{IpAddr, SocketAddr};
    use std::str::FromStr;
    if let Ok(sa) = SocketAddr::from_str(input) {
        return Some(sa.ip());
    }
    let stripped = input
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim()
        .to_string();
    if let Ok(ip) = IpAddr::from_str(&stripped) {
        return Some(ip);
    }
    // Final fallback for IPv4-with-port reported as `1.2.3.4:443`
    // already covered by SocketAddr above; nothing else to try.
    None
}

/// Build a `Status::resource_exhausted`-shaped HTTP response. Per
/// the prompt's hard rule, rate-limit returns `resource_exhausted`
/// (NOT `unavailable`) — the gRPC code maps cleanly to WS close 4001
/// via [`crate::services::ws::map_status_to_close`].
fn rate_limit_response(decision: RateLimitDecision) -> http::Response<Body> {
    let status = tonic::Status::resource_exhausted(decision.reason().to_string());
    grpc_status_response(status)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_with_handle_uses_explicit_jwks() {
        // Sub-phase D (D7): when an explicit `Arc<JwksCache>` is
        // threaded through the middleware, no global state is touched.
        // The dev-only cache accepts the magic Bearer.
        let cache = Arc::new(JwksCache::dev_only());
        let claims = verify_with_handle(Some(&cache), "dev-token-for-direct")
            .expect("explicit handle accepts dev token");
        assert_eq!(claims.user_id, "direct");
    }

    #[test]
    fn verify_with_handle_rejects_invalid_via_explicit_jwks() {
        let cache = Arc::new(JwksCache::dev_only());
        let err = verify_with_handle(Some(&cache), "not-a-jwt-or-dev-token")
            .expect_err("invalid bearer rejected");
        match err {
            crate::error::LifegwError::Auth(_) => {}
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn verify_with_handle_explicit_path_does_not_consult_global() {
        // The explicit handle has its own dev-only cache; even if the
        // process-global was never installed, the handle-based verify
        // succeeds. This is the key behavioural guarantee D7 unlocks
        // — per-test verifier swaps without global mutation.
        let cache = Arc::new(JwksCache::dev_only());
        verify_with_handle(Some(&cache), "dev-token-for-isolated").expect("isolated verify");
    }

    // Sub-phase E sweep (item #10): IPv6-with-port parser.
    #[test]
    fn parse_ip_or_socket_accepts_bracketed_ipv6_with_port() {
        let ip = parse_ip_or_socket("[::1]:443").expect("parse [::1]:443");
        assert!(ip.is_loopback());
        assert!(ip.is_ipv6());
    }

    #[test]
    fn parse_ip_or_socket_accepts_ipv4_with_port() {
        let ip = parse_ip_or_socket("1.2.3.4:443").expect("parse 1.2.3.4:443");
        assert_eq!(
            ip,
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(1, 2, 3, 4))
        );
    }

    #[test]
    fn parse_ip_or_socket_accepts_bare_ipv4() {
        let ip = parse_ip_or_socket("10.0.0.1").expect("parse 10.0.0.1");
        assert_eq!(
            ip,
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1))
        );
    }

    #[test]
    fn parse_ip_or_socket_accepts_bare_ipv6() {
        let ip = parse_ip_or_socket("::1").expect("parse ::1");
        assert!(ip.is_loopback());
        assert!(ip.is_ipv6());
    }

    #[test]
    fn parse_ip_or_socket_accepts_bracketed_bare_ipv6() {
        let ip = parse_ip_or_socket("[::1]").expect("parse [::1]");
        assert!(ip.is_loopback());
    }

    #[test]
    fn parse_ip_or_socket_rejects_garbage() {
        assert!(parse_ip_or_socket("definitely not an ip").is_none());
        assert!(parse_ip_or_socket("").is_none());
    }
}
