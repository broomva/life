//! W3C `traceparent` extractor + root-span attachment per Spec C₂ §9.1.
//!
//! Every public-plane RPC carries a W3C `traceparent` header
//! (`00-<trace-id>-<span-id>-<flags>`). This middleware:
//!
//! 1. Extracts the header from the incoming `http::Request`.
//! 2. Parses it via the canonical
//!    [`opentelemetry::propagation::TextMapPropagator`].
//! 3. Stamps the trace context onto the per-request span via
//!    `tracing_opentelemetry::OpenTelemetrySpanExt::set_parent` so
//!    downstream span tree inherits.
//!
//! Sub-phase E: replaced the prior tracing-subscriber-only shim with a
//! TextMapPropagator-driven path. The propagator is installed globally
//! by `observability::init` so this layer can simply look it up via
//! `opentelemetry::global::get_text_map_propagator`.

use std::collections::HashMap;
use std::task::{Context, Poll};

use http::{HeaderMap, Request};
use opentelemetry::propagation::Extractor;
use tonic::body::Body;
use tower::{Layer, Service};
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Clone, Default)]
pub struct TracePropagationLayer;

impl<S> Layer<S> for TracePropagationLayer {
    type Service = TracePropagation<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TracePropagation { inner }
    }
}

#[derive(Clone)]
pub struct TracePropagation<S> {
    inner: S,
}

/// Adapter that lets the canonical OTel propagator read a tonic/hyper
/// `HeaderMap`.
struct HeaderMapExtractor<'a> {
    map: HashMap<String, &'a str>,
}

impl<'a> HeaderMapExtractor<'a> {
    fn new(headers: &'a HeaderMap) -> Self {
        let map = headers
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_lowercase(), v)))
            .collect();
        Self { map }
    }
}

impl Extractor for HeaderMapExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.map.get(&key.to_lowercase()).copied()
    }
    fn keys(&self) -> Vec<&str> {
        self.map.keys().map(|s| s.as_str()).collect()
    }
}

impl<S> Service<Request<Body>> for TracePropagation<S>
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
        // Sub-phase E: extract via the canonical TextMapPropagator. The
        // global propagator is installed in `observability::init`. If
        // no propagator is registered (logging-only mode) the extracted
        // context is empty and downstream spans become roots — Spec C₂
        // §9.1 graceful degradation.
        let extractor = HeaderMapExtractor::new(req.headers());
        let parent_cx =
            opentelemetry::global::get_text_map_propagator(|prop| prop.extract(&extractor));
        Box::pin(async move {
            let span = tracing::Span::current();
            span.set_parent(parent_cx);
            inner.call(req).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_constructs_without_panic() {
        let _layer = TracePropagationLayer;
    }

    #[test]
    fn header_map_extractor_reads_traceparent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
                .parse()
                .unwrap(),
        );
        let extractor = HeaderMapExtractor::new(&headers);
        let tp = extractor.get("traceparent").expect("traceparent extracted");
        assert!(tp.starts_with("00-"));
    }

    #[test]
    fn header_map_extractor_lowercases_lookup() {
        let mut headers = HeaderMap::new();
        headers.insert("Traceparent", "00-x-x-01".parse().unwrap());
        let extractor = HeaderMapExtractor::new(&headers);
        // tonic/hyper already lowercases header names; verify our
        // adapter handles the canonical form.
        assert!(extractor.get("traceparent").is_some());
    }
}
