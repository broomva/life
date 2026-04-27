//! `/healthz` — liveness probe.
//!
//! Spec C₃ §3.5: returns 200 when the upstream lifed UDS is reachable, 503
//! otherwise. Bypasses auth per LOCKED L4-D7. Sub-phase D adds `/readyz`,
//! `/version`, and `/metrics`.

use std::path::PathBuf;
use std::sync::Arc;

use http::{HeaderValue, Response, StatusCode};
use tonic::body::Body;

/// Probe the upstream lifed UDS by attempting to `connect(2)`. Returns true
/// when the socket accepts a connection (regardless of what handshake the
/// peer chooses to perform — readiness here is "the kernel accepted").
async fn upstream_reachable(path: &PathBuf) -> bool {
    tokio::net::UnixStream::connect(path).await.is_ok()
}

/// Build a `Response` for `/healthz`. Returns 200 + "OK" when reachable,
/// 503 + "lifed unreachable" otherwise.
pub async fn handle(upstream: Arc<PathBuf>) -> Response<Body> {
    if upstream_reachable(upstream.as_ref()).await {
        plain_response(StatusCode::OK, "OK")
    } else {
        plain_response(StatusCode::SERVICE_UNAVAILABLE, "lifed unreachable")
    }
}

fn plain_response(status: StatusCode, body: &'static str) -> Response<Body> {
    let mut resp = Response::new(Body::new(http_body_util::Full::new(
        bytes::Bytes::from_static(body.as_bytes()),
    )));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert("content-type", HeaderValue::from_static("text/plain"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn healthz_returns_200_when_upstream_listening() {
        let dir = TempDir::new().expect("tempdir");
        let socket = dir.path().join("life.sock");
        let _listener = UnixListener::bind(&socket).expect("bind upstream");
        let resp = handle(Arc::new(socket)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn healthz_returns_503_when_upstream_missing() {
        let dir = TempDir::new().expect("tempdir");
        let socket = dir.path().join("missing.sock");
        // No listener bound on this path.
        let resp = handle(Arc::new(socket)).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
