//! Tower middleware that runs Tier-2 capability validation and attaches
//! [`CapabilityClaims`] to the request extensions. Per master spec §L7
//! research synthesis 3.10, this MUST be a tower Layer (NOT a tonic
//! interceptor) so it composes with tracing + load-shed + timeout layers
//! without ordering surprises.

use std::sync::Arc;
use std::task::{Context, Poll};

use http::Request;
use tonic::body::Body;
use tower::{Layer, Service};

use crate::auth::jwks::JwksCache;

#[derive(Clone)]
pub struct AuthLayer {
    jwks: Arc<JwksCache>,
}

impl AuthLayer {
    pub fn new(jwks: Arc<JwksCache>) -> Self {
        Self { jwks }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            jwks: Arc::clone(&self.jwks),
        }
    }
}

#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    jwks: Arc<JwksCache>,
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
        let jwks = Arc::clone(&self.jwks);
        Box::pin(async move {
            // Extract authorization header.
            let claims = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|h| h.strip_prefix("Bearer "))
                .and_then(|tok| jwks.validate(tok).ok());

            // Attach (or default) CapabilityClaims as a request extension.
            //
            // SUB-PHASE A ONLY: Missing/invalid bearer falls through to a
            // default-empty `CapabilityClaims`, which lets handlers proceed
            // with empty `user_id`/`project_id`. This is plan-sanctioned for
            // the dev-signer mock-substrate path (integration tests still
            // attach valid bearers; the daemon is dev-only at this point).
            //
            // SUB-PHASE B5 MUST CHANGE THIS: replace `unwrap_or_default()`
            // with an early-return `Status::unauthenticated` for the public
            // plane. Per Spec C₂ §5.1 step 5, invalid Tier-2 capability
            // tokens MUST be rejected before reaching any handler. Tracked
            // as part of B5 (real ES256 + JWKS verification).
            req.extensions_mut().insert(claims.unwrap_or_default());
            inner.call(req).await
        })
    }
}
