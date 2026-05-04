//! HTTP/JSON wrapper for `Agent.CreateSession` (Stage 3a — May 2026).
//!
//! Why this exists
//! ────────────────
//! lifed exposes `Agent.CreateSession` over tonic UDS. The browser
//! / Vercel-side `LifedWsAgentSessionClient` opens a WebSocket directly
//! to `/v1/agent/stream` — but lifed's `StreamSession` returns
//! `Status::not_found("session not found")` when the sid isn't already
//! in its routing cache. The cache populates only after a successful
//! `Agent.CreateSession` (4-step saga: arcan create_agent + lago
//! open_namespace + haima bind_wallet + anima register_session).
//!
//! In production, the canonical flow is:
//!
//!   client                                               lifegw           lifed
//!     │                                                   │                 │
//!     │  POST /v1/agent/create_session  (Tier-1 bearer)   │                 │
//!     ├──────────────────────────────────────────────────▶│                 │
//!     │                                                   │  CreateSession  │
//!     │                                                   ├────────────────▶│
//!     │                                                   │   {sid, ...}    │
//!     │                                                   │◀────────────────┤
//!     │                {sid, ...}                         │                 │
//!     │◀──────────────────────────────────────────────────┤                 │
//!     │                                                   │                 │
//!     │  WS  /v1/agent/stream?sid=<sid>  (bearer)         │                 │
//!     ├──────────────────────────────────────────────────▶│  StreamSession  │
//!     │                                                   ├────────────────▶│
//!     │  ◀──── AgentEvents from upstream substrates ──────┤                 │
//!
//! This handler is the "HTTP/JSON wrapper" leg — same shape as
//! `/anima/custody/*` (Spec D D-Sub-C). It exists so the broomva.tech
//! Next.js side can call `fetch()` instead of pulling in a Connect /
//! tonic-web client + protobuf codegen for one RPC. When the browser
//! eventually speaks Connect natively, callers can switch to the
//! tonic-web grpc-web codec without changing this handler.
//!
//! Auth
//! ────
//! - Inbound: `Authorization: Bearer <Tier-1 JWS>` — verified against
//!   `JwksCache` (the same cache `AuthLayer` uses, which is bound to
//!   the upstream broomva.tech JWKS in production).
//! - Outbound (to lifed): a freshly-minted Tier-2 cap from
//!   `Tier2Minter`, attached as `authorization: Bearer <jws>` on the
//!   tonic call. Mirrors the AuthLayer's mint flow.
//!
//! Strict shape
//! ────────────
//! `#[serde(deny_unknown_fields)]` — lifegw is a security boundary;
//! reject anything we don't recognise.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use serde::{Deserialize, Serialize};
use tonic::transport::Channel;

use life_runtime_proto::life::v1::{self as pb, agent_client::AgentClient};

use crate::auth::jwks::JwksCache;
use crate::auth::tier2::Tier2Minter;

/// Per-RPC deadline for the upstream `lifed.Agent.CreateSession` call.
/// Same posture as the anima_custody routes — generous for happy-path
/// (the saga is single-digit ms against in-process mocks; ~100 ms
/// against real substrates) while keeping the route handler responsive
/// under upstream brownout.
const UPSTREAM_RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Max length of `user_id` / `project_id` / `label` accepted at the
/// gateway boundary. Mirrors the anima_custody bound; anything longer
/// than this is almost certainly malformed input.
const MAX_FIELD_LEN: usize = 128;

/// Shared state threaded into the axum router.
#[derive(Clone)]
pub struct AgentHttpState {
    /// Tier-1 verifier — same handle `AuthLayer` uses.
    pub jwks: Arc<JwksCache>,
    /// Tier-2 minter — same handle `AuthLayer` + `WsLayer` use.
    pub minter: Arc<Tier2Minter>,
    /// Pre-dialed lifed UDS channel. Cheap to clone (internally Arc'd).
    pub upstream: Channel,
}

/// Router mounted at the top-level `/v1/agent/create_session` path.
///
/// We use `Router::route` (exact match) rather than `Router::nest`
/// because `/v1/agent/stream` is handled by the WS layer in the tonic
/// stack — nesting at `/v1/agent` would 404 for paths the inner router
/// doesn't know about. Exact-route match leaves all other `/v1/agent/*`
/// paths to the tonic-stack fallback.
pub fn router(state: AgentHttpState) -> Router {
    Router::new()
        .route("/v1/agent/create_session", post(create_session_handler))
        .with_state(state)
}

// ─── Wire shapes ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionBody {
    pub user_id: String,
    pub project_id: String,
    /// Optional human-readable label — propagated to lifed.
    #[serde(default)]
    pub label: String,
    /// Optional sid to resume from. When set, lifed re-attaches the
    /// existing session rather than running the create-session saga
    /// again.
    #[serde(default)]
    pub resume_sid: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResp {
    pub sid: String,
    pub agent_id: String,
    pub user_id: String,
    pub project_id: String,
    /// Unix-seconds `created_at` from lifed.
    pub created_at_unix: i64,
}

#[derive(Debug, Serialize)]
struct ErrJson {
    error: String,
}

// ─── Handler ────────────────────────────────────────────────────────────

async fn create_session_handler(
    State(state): State<AgentHttpState>,
    headers: HeaderMap,
    Json(body): Json<CreateSessionBody>,
) -> Result<Json<CreateSessionResp>, (StatusCode, Json<ErrJson>)> {
    // 1. Tier-1 verify (same flow as AuthLayer).
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrJson {
                    error: "missing Tier-1 bearer".to_string(),
                }),
            )
        })?;
    let tier1 = state.jwks.verify(bearer).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrJson {
                error: format!("invalid Tier-1: {e}"),
            }),
        )
    })?;

    // 2. Validate body shape. `deny_unknown_fields` catches typos; we
    //    additionally bound field lengths so a buggy / malicious caller
    //    can't fill the routing cache with multi-MB user_ids.
    if body.user_id.is_empty() || body.project_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrJson {
                error: "user_id and project_id required".to_string(),
            }),
        ));
    }
    if body.user_id.len() > MAX_FIELD_LEN
        || body.project_id.len() > MAX_FIELD_LEN
        || body.label.len() > MAX_FIELD_LEN
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrJson {
                error: format!("user_id / project_id / label must be ≤ {MAX_FIELD_LEN} characters"),
            }),
        ));
    }
    // sub/user_id binding (same pattern as anima_custody): the Tier-1
    // bearer's subject MUST match the body's user_id. Without this, a
    // Tier-1 cap minted for user X could be replayed to create a
    // session for user Y.
    if tier1.user_id != body.user_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrJson {
                error: format!(
                    "claims.sub `{}` does not match body.user_id `{}`",
                    tier1.user_id, body.user_id
                ),
            }),
        ));
    }

    // 3. Mint Tier-2.
    let tier2 = state.minter.mint(&tier1).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrJson {
                error: format!("mint tier-2: {e}"),
            }),
        )
    })?;

    // 4. Forward to lifed with the Tier-2 cap.
    let mut client = AgentClient::new(state.upstream.clone());
    let mut tonic_req = tonic::Request::new(pb::CreateSessionReq {
        user_id: body.user_id.clone(),
        project_id: body.project_id.clone(),
        label: body.label.clone(),
        resume_sid: body
            .resume_sid
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| aios_proto::aios::v1::SessionId {
                value: s.to_string(),
            }),
        inherit_policy: None,
    });
    let bearer_value: tonic::metadata::MetadataValue<_> = format!("Bearer {tier2}")
        .parse()
        .map_err(|e: tonic::metadata::errors::InvalidMetadataValue| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrJson {
                    error: format!("encode tier-2 metadata: {e}"),
                }),
            )
        })?;
    tonic_req
        .metadata_mut()
        .insert("authorization", bearer_value);

    let upstream_call = client.create_session(tonic_req);
    let resp = match tokio::time::timeout(UPSTREAM_RPC_TIMEOUT, upstream_call).await {
        Ok(Ok(r)) => r,
        Ok(Err(status)) => {
            return Err((
                map_tonic_to_http(status.code()),
                Json(ErrJson {
                    error: sanitize_upstream(status.message()),
                }),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                Json(ErrJson {
                    error: "lifed.Agent.CreateSession exceeded 10s deadline".to_string(),
                }),
            ));
        }
    };

    let sess = resp.into_inner();
    let sid = sess.sid.map(|s| s.value).unwrap_or_default();
    let agent_id = sess.agent_id.map(|a| a.value).unwrap_or_default();
    let created_at_unix = sess.created_at.map(|t| t.seconds).unwrap_or_default();
    Ok(Json(CreateSessionResp {
        sid,
        agent_id,
        user_id: sess.user_id,
        project_id: sess.project_id,
        created_at_unix,
    }))
}

// ─── Helpers ────────────────────────────────────────────────────────────

/// Map the upstream tonic `Code` to the closest HTTP status. Mirrors
/// the close-code mapping in `services::ws::map_status_to_close` but
/// translated to HTTP for unary callers.
fn map_tonic_to_http(code: tonic::Code) -> StatusCode {
    use tonic::Code;
    match code {
        Code::Unauthenticated | Code::PermissionDenied => StatusCode::UNAUTHORIZED,
        Code::InvalidArgument | Code::FailedPrecondition => StatusCode::BAD_REQUEST,
        Code::NotFound => StatusCode::NOT_FOUND,
        Code::AlreadyExists => StatusCode::CONFLICT,
        Code::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        Code::Unavailable => StatusCode::BAD_GATEWAY,
        Code::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Sanitise upstream error messages so we don't leak internal-only
/// details (UDS paths, sql fragments, etc.) into the public response.
/// Same posture as `lifegw::services::anima_custody::sanitize_upstream`.
fn sanitize_upstream(msg: &str) -> String {
    if msg.is_empty() {
        return "upstream lifed call failed".to_string();
    }
    // Strip anything that looks like a filesystem path (UDS paths,
    // private-state paths, etc.). Conservative — anything starting with
    // `/` is replaced with a placeholder.
    let cleaned: String = msg
        .split_whitespace()
        .map(|tok| {
            if tok.starts_with('/') {
                "<path-redacted>"
            } else {
                tok
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    // Bound the length to keep the response payload small.
    if cleaned.len() > 256 {
        format!("{}…", &cleaned[..256])
    } else {
        cleaned
    }
}

// Used by the IntoResponse impl for axum's `Result<_, (StatusCode,
// Json<_>)>`. The closure returns are `(StatusCode, Json<ErrJson>)`
// directly; this trait impl is here only to keep the handler's return
// type signature concise should we need to wrap it.
#[doc(hidden)]
fn _impl_response_compile_check() {
    fn _check<T: IntoResponse>() {}
    _check::<(StatusCode, Json<ErrJson>)>();
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwks::{JwksCache, JwksCacheConfig, JwksSource};
    use crate::auth::kms::StaticKeystore;
    use crate::config::AuthConfig;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use jsonwebtoken::{Algorithm, Header, encode};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::util::ServiceExt;

    /// Mint a Tier-1-shaped JWT that the dev JwksCache accepts via the
    /// `dev-token-for-{user_id}` shortcut. Used so we don't need to
    /// stand up the full real-JWS verifier in unit tests.
    fn dev_tier1_token(user: &str) -> String {
        format!("dev-token-for-{user}")
    }

    /// Mint a real ES256 Tier-1 JWS signed by the same dev keystore the
    /// JwksCache loads. Mirrors what broomva.tech's `mintTier1ForConsumer`
    /// produces; lets us exercise the full verify path in tests.
    fn real_tier1_jws(user: &str, ks: &crate::auth::keystore::Keystore) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(ks.kid.clone());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let body = serde_json::json!({
            "iss": "https://broomva.tech",
            "aud": "lifegw",
            "sub": user,
            "scopes": ["agent:dispatch"],
            "tier": "free",
            "iat": now,
            "nbf": now,
            "exp": now + 600,
        });
        encode(&header, &body, &ks.encoding).expect("encode tier-1")
    }

    /// Helper: assemble a state with the dev cache (accepts shortcut)
    /// and a static keystore minter. The `upstream` channel is dialed
    /// to a non-existent UDS path so any successful tonic call attempt
    /// would fail with `Unavailable` — tests that care about the auth
    /// path stop at body validation / Tier-1 verify before reaching
    /// upstream.
    async fn dev_state() -> AgentHttpState {
        let jwks = Arc::new(JwksCache::dev_only());
        let signer = Arc::new(StaticKeystore::generate_dev().expect("keystore"));
        let minter = Arc::new(Tier2Minter::new(signer, &AuthConfig::default()));
        // Build a Channel that points at a definitely-unreachable UDS.
        // We never make a successful upstream call in these tests —
        // the goal is to assert auth + body validation behaviour.
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").unwrap();
        let channel = endpoint.connect_with_connector_lazy(tower::service_fn(
            |_: tonic::transport::Uri| async {
                Err::<hyper_util::rt::TokioIo<tokio::net::UnixStream>, std::io::Error>(
                    std::io::Error::other("unreachable in unit tests"),
                )
            },
        ));
        AgentHttpState {
            jwks,
            minter,
            upstream: channel,
        }
    }

    /// POST a JSON body to /v1/agent/create_session via the in-memory
    /// router. Returns (status, body).
    async fn post_create_session(
        state: AgentHttpState,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (StatusCode, String) {
        let app = router(state);
        let mut req_builder = Request::builder()
            .method("POST")
            .uri("/v1/agent/create_session")
            .header("content-type", "application/json");
        for (k, v) in headers {
            req_builder = req_builder.header(*k, *v);
        }
        let req = req_builder
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.expect("handler oneshot");
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 4096).await.expect("body");
        (
            status,
            String::from_utf8(bytes.to_vec()).unwrap_or_default(),
        )
    }

    #[tokio::test]
    async fn rejects_missing_authorization_header() {
        let state = dev_state().await;
        let (status, body) =
            post_create_session(state, &[], r#"{"user_id":"alice","project_id":"sentinel"}"#).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("missing Tier-1 bearer"), "body: {body}");
    }

    #[tokio::test]
    async fn rejects_invalid_tier1() {
        let state = dev_state().await;
        let (status, body) = post_create_session(
            state,
            &[("authorization", "Bearer not-a-token")],
            r#"{"user_id":"alice","project_id":"sentinel"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("invalid Tier-1"), "body: {body}");
    }

    #[tokio::test]
    async fn rejects_empty_user_id() {
        let state = dev_state().await;
        let auth = format!("Bearer {}", dev_tier1_token("alice"));
        let (status, body) = post_create_session(
            state,
            &[("authorization", auth.as_str())],
            r#"{"user_id":"","project_id":"sentinel"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("required"), "body: {body}");
    }

    #[tokio::test]
    async fn rejects_unknown_field() {
        let state = dev_state().await;
        let auth = format!("Bearer {}", dev_tier1_token("alice"));
        let (status, _body) = post_create_session(
            state,
            &[("authorization", auth.as_str())],
            r#"{"user_id":"alice","project_id":"sentinel","sneaky":true}"#,
        )
        .await;
        // serde rejects the unknown field at deserialize time → axum
        // returns 422 Unprocessable Entity (its default for json
        // extraction errors).
        assert!(
            status == StatusCode::UNPROCESSABLE_ENTITY || status == StatusCode::BAD_REQUEST,
            "unexpected status {status}",
        );
    }

    #[tokio::test]
    async fn rejects_oversized_field() {
        let state = dev_state().await;
        let auth = format!("Bearer {}", dev_tier1_token("alice"));
        let oversized = "x".repeat(MAX_FIELD_LEN + 1);
        let body_json = format!(r#"{{"user_id":"alice","project_id":"{oversized}"}}"#);
        let (status, body) =
            post_create_session(state, &[("authorization", auth.as_str())], &body_json).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("characters"), "body: {body}");
    }

    #[tokio::test]
    async fn rejects_subject_mismatch() {
        // Tier-1 cap minted for "alice"; body claims "bob" — must reject.
        let state = dev_state().await;
        let auth = format!("Bearer {}", dev_tier1_token("alice"));
        let (status, body) = post_create_session(
            state,
            &[("authorization", auth.as_str())],
            r#"{"user_id":"bob","project_id":"sentinel"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("does not match"), "body: {body}");
    }

    #[tokio::test]
    async fn happy_path_reaches_upstream_call() {
        // The dev verifier accepts the shortcut; we mint Tier-2 and
        // attempt the upstream call. The upstream is unreachable in
        // these tests, so we expect a 502 (Unavailable mapped). What
        // matters is that we got past auth + validation cleanly.
        let state = dev_state().await;
        let auth = format!("Bearer {}", dev_tier1_token("alice"));
        let (status, body) = post_create_session(
            state,
            &[("authorization", auth.as_str())],
            r#"{"user_id":"alice","project_id":"sentinel","label":"smoke"}"#,
        )
        .await;
        // Either Bad Gateway (Unavailable upstream) or Gateway Timeout
        // (the lazy connector might park) — both prove auth + minting
        // succeeded and the route attempted the upstream tonic call.
        assert!(
            status == StatusCode::BAD_GATEWAY
                || status == StatusCode::GATEWAY_TIMEOUT
                || status == StatusCode::INTERNAL_SERVER_ERROR,
            "unexpected status {status}, body: {body}",
        );
    }

    #[tokio::test]
    async fn happy_path_with_real_jws_passes_auth() {
        // Build a JwksCache that holds the dev keystore's public key,
        // mint a real ES256 JWS, present it. Same as a production
        // broomva.tech flow.
        use crate::auth::jwks::{JwksDoc, JwksEntry};
        let signer_ks = crate::auth::keystore::Keystore::generate_dev().expect("ks");
        let jwks_doc = JwksDoc::new(vec![JwksEntry::ec_p256_pem(
            signer_ks.kid.clone(),
            signer_ks.public_key_pem(),
        )]);
        let jwks_cfg = JwksCacheConfig::new(
            JwksSource::Inline(jwks_doc),
            "lifegw",
            "https://broomva.tech",
        );
        let jwks = Arc::new(JwksCache::new(jwks_cfg));
        let signer_for_minter = Arc::new(StaticKeystore::generate_dev().expect("ks2"));
        let minter = Arc::new(Tier2Minter::new(signer_for_minter, &AuthConfig::default()));
        // Lazy unreachable upstream — we don't care about reaching it,
        // only that auth + body validation pass.
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").unwrap();
        let upstream = endpoint.connect_with_connector_lazy(tower::service_fn(
            |_: tonic::transport::Uri| async {
                Err::<hyper_util::rt::TokioIo<tokio::net::UnixStream>, std::io::Error>(
                    std::io::Error::other("unreachable"),
                )
            },
        ));
        let state = AgentHttpState {
            jwks,
            minter,
            upstream,
        };

        let token = real_tier1_jws("alice", &signer_ks);
        let auth = format!("Bearer {token}");
        let (status, body) = post_create_session(
            state,
            &[("authorization", auth.as_str())],
            r#"{"user_id":"alice","project_id":"sentinel"}"#,
        )
        .await;
        // We expect to reach the upstream call (which fails) → BAD_GATEWAY.
        // Any auth-related failure would surface as 401/403 instead.
        assert!(
            status == StatusCode::BAD_GATEWAY
                || status == StatusCode::GATEWAY_TIMEOUT
                || status == StatusCode::INTERNAL_SERVER_ERROR,
            "unexpected status {status}, body: {body}",
        );
        assert!(
            !body.contains("invalid Tier-1") && !body.contains("missing Tier-1"),
            "auth should have passed, body: {body}",
        );
    }

    #[test]
    fn map_tonic_codes() {
        assert_eq!(
            map_tonic_to_http(tonic::Code::Unauthenticated),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::PermissionDenied),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::InvalidArgument),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::NotFound),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::AlreadyExists),
            StatusCode::CONFLICT
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::ResourceExhausted),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::Unavailable),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::DeadlineExceeded),
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::Internal),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn sanitize_redacts_paths() {
        let s = sanitize_upstream("connect failed at /run/life/life.sock with EADDRNOTAVAIL");
        assert!(!s.contains("/run/life"), "path should be redacted: {s}");
        assert!(s.contains("EADDRNOTAVAIL"));
    }

    #[test]
    fn sanitize_bounds_length() {
        let long_msg = "x".repeat(1000);
        let s = sanitize_upstream(&long_msg);
        // 256 ASCII bytes + 3-byte `…` (U+2026 in UTF-8) = 259 bytes max.
        assert!(s.len() <= 259, "len={} (expected ≤ 259)", s.len());
        assert!(s.ends_with('…'), "tail elided");
        assert_eq!(s.chars().count(), 257, "256 chars + 1 ellipsis char");
    }

    #[test]
    fn sanitize_handles_empty() {
        let s = sanitize_upstream("");
        assert!(s.contains("upstream"), "fallback message: {s}");
    }

    /// Compile-time guard so the helper trait check stays linked into
    /// the test binary.
    #[test]
    fn response_compile_check() {
        _impl_response_compile_check();
    }
}
