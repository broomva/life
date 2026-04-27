//! Tower middleware that runs Tier-2 capability validation and attaches
//! [`CapabilityClaims`] to the request extensions. Per master spec §L7
//! research synthesis 3.10, this MUST be a tower Layer (NOT a tonic
//! interceptor) so it composes with tracing + load-shed + timeout layers
//! without ordering surprises.
//!
//! Per Spec C₂ §5.1 step 5, an invalid or missing Tier-2 bearer token MUST
//! be rejected with `Status::unauthenticated` BEFORE reaching any handler.
//! Sub-phase B implements that early-return directly here; sub-phase A's
//! `unwrap_or_default()` lenience is gone.

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
            let bearer = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|h| h.strip_prefix("Bearer "))
                .map(|t| t.to_string());

            let claims = match bearer {
                Some(tok) => match jwks.validate(&tok) {
                    Ok(c) => c,
                    Err(_) => return Ok(unauth_response("invalid Tier-2 capability token")),
                },
                None => return Ok(unauth_response("missing Tier-2 capability token")),
            };

            req.extensions_mut().insert(claims);
            inner.call(req).await
        })
    }
}

/// Build a `Status::unauthenticated`-shaped HTTP response. Per Spec C₂
/// §5.1 step 5, we never let the request reach a handler when auth fails.
/// Tonic clients surface the response as `tonic::Status` of code
/// `Unauthenticated` because we set the `grpc-status` trailer header.
fn unauth_response(msg: &str) -> http::Response<Body> {
    let status = tonic::Status::unauthenticated(msg.to_string());
    let mut resp = http::Response::new(Body::empty());
    *resp.status_mut() = http::StatusCode::OK;
    let headers = resp.headers_mut();
    headers.insert(
        "content-type",
        http::HeaderValue::from_static("application/grpc"),
    );
    headers.insert(
        "grpc-status",
        http::HeaderValue::from_str(&(status.code() as i32).to_string()).unwrap(),
    );
    if let Ok(v) = http::HeaderValue::from_str(status.message()) {
        headers.insert("grpc-message", v);
    }
    resp
}
