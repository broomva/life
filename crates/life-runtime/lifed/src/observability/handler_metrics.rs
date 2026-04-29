//! `life.daemon.handler.duration_ms` middleware per Spec C₂ §9.3.
//!
//! Sub-phase E: a small tower::Layer that times each public-plane RPC
//! handler invocation and records the elapsed milliseconds against the
//! `handler_duration_ms{namespace,method}` histogram. The (namespace,
//! method) labels are derived from the gRPC URL path
//! (`/life.v1.<Service>/<Method>`).

use std::task::{Context, Poll};
use std::time::Instant;

use http::Request;
use tonic::body::Body;
use tower::{Layer, Service};

#[derive(Clone, Default)]
pub struct HandlerMetricsLayer;

impl<S> Layer<S> for HandlerMetricsLayer {
    type Service = HandlerMetrics<S>;
    fn layer(&self, inner: S) -> Self::Service {
        HandlerMetrics { inner }
    }
}

#[derive(Clone)]
pub struct HandlerMetrics<S> {
    inner: S,
}

fn parse_path(path: &str) -> (String, String) {
    // gRPC paths are of the form `/<package>.<Service>/<Method>`.
    // Map them to (namespace, method) where namespace = the trailing
    // service name lower-cased (`agent`, `events`, `wallet`, ...).
    let trimmed = path.trim_start_matches('/');
    let mut parts = trimmed.splitn(2, '/');
    let svc = parts.next().unwrap_or("");
    let method = parts.next().unwrap_or("").to_string();
    let namespace = svc.rsplit('.').next().unwrap_or("").to_lowercase();
    (namespace, method)
}

impl<S> Service<Request<Body>> for HandlerMetrics<S>
where
    S: Service<Request<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        let (namespace, method) = parse_path(req.uri().path());
        let started = Instant::now();
        Box::pin(async move {
            let res = inner.call(req).await;
            let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            super::metrics::record_handler_duration_ms(&namespace, &method, elapsed_ms);
            res
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_path_extracts_namespace_and_method() {
        let (ns, m) = parse_path("/life.v1.Agent/CreateSession");
        assert_eq!(ns, "agent");
        assert_eq!(m, "CreateSession");
    }

    #[test]
    fn parse_path_handles_admin_namespace() {
        let (ns, m) = parse_path("/life.admin.v1.Saga/ListInflight");
        assert_eq!(ns, "saga");
        assert_eq!(m, "ListInflight");
    }

    #[test]
    fn parse_path_tolerates_malformed_input() {
        // No `/` separator after the service token — namespace falls back
        // to the whole token, method is empty.
        let (ns, m) = parse_path("/garbage");
        assert_eq!(ns, "garbage");
        assert!(m.is_empty());
    }

    #[test]
    fn parse_path_handles_empty_input() {
        let (ns, m) = parse_path("");
        assert!(ns.is_empty());
        assert!(m.is_empty());
    }
}
