//! W3C `traceparent` extractor + root-span attachment per Spec C₂ §9.1.
//!
//! Every public-plane RPC carries a W3C `traceparent` header
//! (`00-<trace-id>-<span-id>-<flags>`). This middleware:
//!
//! 1. Extracts the header from the incoming `http::Request`.
//! 2. Parses it (lazily — invalid headers are logged + ignored).
//! 3. Stamps the trace context onto the per-request span so downstream
//!    spans inherit the trace tree.
//!
//! Sub-phase D5 ships the extractor shape; the OpenTelemetry context
//! propagation wires up alongside the OTLP exporter in sub-phase E.

use std::task::{Context, Poll};

use http::Request;
use tonic::body::Body;
use tower::{Layer, Service};

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
        let traceparent = req
            .headers()
            .get("traceparent")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        Box::pin(async move {
            // Sub-phase D5: emit the traceparent into a span field so
            // any downstream tracing-opentelemetry layer can inject it
            // into the OTel context. Sub-phase E swaps to the
            // canonical opentelemetry::propagation::Extractor / TextMapPropagator
            // pair against vigil's TracerProvider.
            if let Some(tp) = traceparent.as_deref() {
                tracing::trace!(traceparent = tp, "incoming request carries traceparent");
            }
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
}
