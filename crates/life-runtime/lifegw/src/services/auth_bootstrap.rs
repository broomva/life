//! Phase α₁ — Lumen credential-bootstrap endpoint.
//!
//! Mints a Tier-User capability token for a local Lumen launch so
//! `scripts/lumen-app.sh` can hand it to the warp-oss process via the
//! `WARP_USER_SECRET` env var (consumed by upstream
//! `app/src/auth/auth_state.rs:133-138`). Lumen wraps the value as
//! `Credentials::ApiKey` / `Credentials::Bearer` and attaches
//! `Authorization: Bearer <token>` on every subsequent call,
//! bypassing Firebase entirely.
//!
//! This is the foundational endpoint of the Life-native wiring path
//! documented in `docs/lumen/architecture/2026-05-15-life-native-wiring.md`
//! §"Phase α₁ in detail".
//!
//! ## Wire shape
//!
//! ```text
//! POST /life/v1/auth/bootstrap
//! Content-Type: application/json
//! Body:    { "user_id": "lumen-local", "client_label"?: "Lumen GUI" }
//! Returns: 200 OK
//!          { "access_token": "<JWS>",
//!            "token_type":   "Bearer",
//!            "user_id":      "lumen-local",
//!            "expires_in":   900,
//!            "expires_at":   1736982000 }
//! ```
//!
//! ## Security posture
//!
//! - **Local-dev only**: this endpoint is exposed on the public
//!   axum router (same as `/graphql/v2`) but mints a Tier-User cap
//!   scoped to `["lumen:client", "agent:dispatch"]`. The scope set
//!   is intentionally narrow so a leaked token can only drive Lumen
//!   client surfaces, not admin-plane operations.
//! - **No password / proof-of-possession**: bootstrap is anonymous
//!   in α₁ because Lumen runs co-located with lifegw on the same
//!   host. Production deployments will gate this with the existing
//!   rate-limiter (`crate::services::rate_limit`) + a per-host
//!   Anima identity binding (Phase α₂ folds anima identity in).
//! - **Token TTL**: 15 minutes (`DEFAULT_TIER_USER_TTL`). Lumen
//!   refreshes by calling bootstrap again — `scripts/lumen-app.sh`
//!   currently bootstraps once per launch; a long-lived Lumen
//!   session would need a refresh loop, tracked as α₁-follow-up.

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use serde::{Deserialize, Serialize};

use crate::auth::tier_user::TierUserMinter;

/// State for the bootstrap route. Carries:
///
/// - `minter`: a `TierUserMinter` configured with audience `"lumen"`
///   so the minted JWS passes `warp_graphql`'s verifier in production
///   (where `JwksSource::Url(...)` points at an external JWKS that
///   includes the active kid).
/// - `dev_signer_enabled`: when `true`, the bootstrap endpoint emits
///   a `dev-cap-token-for-lumen/{user_id}` **dev-shortcut** token
///   instead of a real JWS. This is the correct path for local
///   dev because the warp_graphql route's outer JwksCache is built
///   with `JwksSource::Url(cfg.jwks_url)` (Vercel-style), which
///   does NOT include our fresh in-process dev signer's kid. The
///   verifier's dev-shortcut path (gated to `dev_signer_enabled`)
///   accepts the magic prefix without a JWKS lookup. See
///   `verify_capability_token` in `auth/jwks.rs:626`.
#[derive(Clone)]
pub struct AuthBootstrapState {
    pub minter: Arc<TierUserMinter>,
    pub dev_signer_enabled: bool,
}

/// Default scopes minted for a Lumen client. Narrow enough that a
/// leaked token can't drive admin-plane RPCs but wide enough that
/// every first-launch GraphQL/REST call Lumen makes succeeds.
const LUMEN_DEFAULT_SCOPES: &[&str] = &[
    "lumen:client",
    "agent:dispatch",
    "agent:read",
    "events:read",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapRequest {
    /// Stable identifier for this Lumen launch. `scripts/lumen-app.sh`
    /// sets it to a synthesized value like `lumen-{hostname}`. Maps
    /// into the JWS `sub` claim.
    pub user_id: String,
    /// Optional human-readable label propagated nowhere yet —
    /// reserved for telemetry / audit log when those land. Phase α₁
    /// accepts but ignores it.
    #[serde(default)]
    pub client_label: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BootstrapResponse {
    /// Compact-form JWS signed by lifegw's KMS signer. Lumen attaches
    /// this verbatim as `Authorization: Bearer <token>`.
    pub access_token: String,
    /// Always `"Bearer"`. Spelled out so Lumen's HTTP client doesn't
    /// have to guess. Owned `String` (not `&'static`) so callers can
    /// deserialize the response in tests / integration suites.
    pub token_type: String,
    /// Echoed back so the launcher script can record it in
    /// `/tmp/lumen-app/warp-oss.log` for debuggability.
    pub user_id: String,
    /// Seconds until the token expires. Lumen's existing
    /// `refresh_access_token` logic ignores this for `Credentials::ApiKey`,
    /// but we surface it for any future refresh-loop integration.
    pub expires_in: u64,
    /// Absolute unix-timestamp expiry. Same value, different shape —
    /// human-readable shell scripts use this; programmatic consumers
    /// use `expires_in`.
    pub expires_at: i64,
}

/// Router factory.
pub fn router(state: AuthBootstrapState) -> Router {
    Router::new()
        .route("/life/v1/auth/bootstrap", post(bootstrap_handler))
        .with_state(state)
}

async fn bootstrap_handler(
    State(state): State<AuthBootstrapState>,
    Json(req): Json<BootstrapRequest>,
) -> Result<Json<BootstrapResponse>, AuthBootstrapError> {
    // Reject empty user_id early — the JWS spec allows empty `sub`
    // but every downstream consumer treats it as a malformed claim.
    let user_id = req.user_id.trim();
    if user_id.is_empty() {
        return Err(AuthBootstrapError::InvalidUserId);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    if state.dev_signer_enabled {
        // Dev-shortcut path: emit `dev-cap-token-for-lumen/{user_id}`.
        // The warp_graphql verifier's `verify_capability_token` accepts
        // this prefix when `dev_signer_enabled` is true, bypassing the
        // JWKS lookup that would otherwise fail (the outer JwksCache
        // uses `JwksSource::Url(cfg.jwks_url)` which doesn't contain
        // our in-process dev signer's kid). The TTL is effectively
        // unbounded server-side because there's no JWT-style `exp`
        // field on a dev-shortcut token; we surface the same 15-min
        // window the production path would use so launcher scripts
        // can treat both paths uniformly.
        let token = format!("dev-cap-token-for-lumen/{user_id}");
        let exp = now + (15 * 60);
        let expires_in = 15 * 60u64;
        return Ok(Json(BootstrapResponse {
            access_token: token,
            token_type: "Bearer".to_string(),
            user_id: user_id.to_string(),
            expires_in,
            expires_at: exp,
        }));
    }

    // Production path: mint a real Tier-User JWS with audience
    // `"lumen"`. The signer's kid MUST be present in the warp_graphql
    // route's JwksCache (typically via `JwksSource::Url` pointing at a
    // production JWKS that publishes the same key material lifegw's
    // signer holds).
    let scopes: Vec<String> = LUMEN_DEFAULT_SCOPES
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let (token, exp) = state
        .minter
        .mint(user_id, scopes)
        .map_err(|e| AuthBootstrapError::Mint(e.to_string()))?;
    let expires_in = (exp - now).max(0) as u64;

    Ok(Json(BootstrapResponse {
        access_token: token,
        token_type: "Bearer".to_string(),
        user_id: user_id.to_string(),
        expires_in,
        expires_at: exp,
    }))
}

/// Error responses surfaced as JSON `{ "error": "...", "code": "..." }`
/// with the matching HTTP status. Lumen's bootstrap loop doesn't
/// branch on these codes today, but keeping them stable makes the
/// surface forward-compatible.
#[derive(Debug)]
pub enum AuthBootstrapError {
    InvalidUserId,
    Mint(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    code: &'static str,
}

impl IntoResponse for AuthBootstrapError {
    fn into_response(self) -> axum::response::Response {
        let (status, body) = match self {
            AuthBootstrapError::InvalidUserId => (
                StatusCode::BAD_REQUEST,
                ErrorBody {
                    error: "user_id must be a non-empty string".to_string(),
                    code: "INVALID_USER_ID",
                },
            ),
            AuthBootstrapError::Mint(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody {
                    error: format!("token mint failed: {msg}"),
                    code: "MINT_FAILED",
                },
            ),
        };
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::kms::StaticKeystore;
    use crate::auth::tier_user::{DEFAULT_TIER_USER_TTL, TierUserMinter};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    fn test_state_prod() -> AuthBootstrapState {
        let signer = Arc::new(StaticKeystore::generate_dev().expect("dev keystore"));
        let minter = Arc::new(TierUserMinter::with_defaults(signer, DEFAULT_TIER_USER_TTL));
        AuthBootstrapState {
            minter,
            dev_signer_enabled: false,
        }
    }

    fn test_state_dev() -> AuthBootstrapState {
        let mut s = test_state_prod();
        s.dev_signer_enabled = true;
        s
    }

    fn test_state() -> AuthBootstrapState {
        test_state_prod()
    }

    #[tokio::test]
    async fn happy_path_prod_returns_signed_jws() {
        let app = router(test_state_prod());
        let body = serde_json::json!({"user_id": "lumen-local"}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/life/v1/auth/bootstrap")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: BootstrapResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.token_type, "Bearer");
        assert_eq!(parsed.user_id, "lumen-local");
        assert!(parsed.access_token.contains('.'), "token must be a JWS");
        assert!(parsed.expires_in > 0);
        assert!(parsed.expires_at > 0);
    }

    #[tokio::test]
    async fn happy_path_dev_returns_shortcut_token() {
        let app = router(test_state_dev());
        let body = serde_json::json!({"user_id": "lumen-local"}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/life/v1/auth/bootstrap")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: BootstrapResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.token_type, "Bearer");
        assert_eq!(parsed.user_id, "lumen-local");
        assert_eq!(
            parsed.access_token, "dev-cap-token-for-lumen/lumen-local",
            "dev mode must emit the verify_capability_token shortcut shape"
        );
        assert_eq!(parsed.expires_in, 15 * 60);
    }

    #[tokio::test]
    async fn empty_user_id_rejected_400() {
        let app = router(test_state());
        let body = serde_json::json!({"user_id": ""}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/life/v1/auth/bootstrap")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn whitespace_only_user_id_rejected_400() {
        let app = router(test_state());
        let body = serde_json::json!({"user_id": "   "}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/life/v1/auth/bootstrap")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_field_rejected_due_to_deny_unknown_fields() {
        let app = router(test_state());
        let body = serde_json::json!({"user_id": "x", "haxx": 1}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/life/v1/auth/bootstrap")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        // axum's JSON extractor maps a serde unknown-field error to 422.
        assert!(
            resp.status() == StatusCode::UNPROCESSABLE_ENTITY
                || resp.status() == StatusCode::BAD_REQUEST,
            "got {}",
            resp.status()
        );
    }
}
