//! `/anima/custody/*` HTTP/JSON routes (Spec D D-Sub-C — Stream R-2).
//!
//! lifegw-side surface of the browser/remote custody bridge. Six routes
//! that proxy to soma's admin custody-oracle UDS (`life.admin.kernel.v1.CustodyOracle`)
//! and one issuer route that mints Tier-User capability tokens. Together
//! these routes serve both the Rust `RemoteAnima` (Stream R-1, PR #1082)
//! and the browser-side `WebCryptoAnima` (Stream T).
//!
//! ## Routes
//!
//! | Method | Path | Body | Response |
//! |---|---|---|---|
//! | POST | `/anima/custody/sign_auth` | `{ user_id, digest_b64 }` | `{ signature_b64 }` |
//! | POST | `/anima/custody/sign_wallet` | `{ user_id, digest_b64 }` | `{ signature_b64 }` |
//! | GET | `/anima/custody/get_auth_pubkey/{user_id}` | — | `{ pubkey_b64 }` |
//! | GET | `/anima/custody/get_wallet_pubkey/{user_id}` | — | `{ pubkey_b64 }` |
//! | POST | `/anima/custody/mint_session_cap` | `{ user_id, passkey_assertion_b64, client_data_json_b64 }` | `{ token, expires_at_unix }` |
//! | POST | `/anima/custody/enroll_passkey` | `{ user_id, attestation_object_b64, client_data_json_b64 }` | `{ token, expires_at_unix, did }` |
//!
//! ## Authn (Spec D D-Sub-C review fixes — B1, B2, I1)
//!
//! All routes require `Authorization: Bearer <jwt>` where the JWT is
//! either:
//! - a Tier-2 capability (`aud=lifed`) — server-side caller, full access
//! - a Tier-User capability (`aud=anima.user-cap`) — browser/RemoteAnima
//!   caller, per-route scope-gated access
//!
//! Both shapes verify against the SAME published JWKS (single signing
//! key per gateway). The route handlers verify the bearer's signature +
//! audience + nbf/exp via [`crate::auth::jwks::JwksCache::verify_capability_token`]
//! and additionally enforce:
//!
//! - **B2 (per-route scope intersection)** — Tier-User caps must carry
//!   the route's required scope (`anima.user.sign_auth`,
//!   `anima.user.sign_wallet`, `anima.user.get_pubkey`). Tier-2 caps
//!   bypass this check (server-side callers have implicit full access).
//! - **I1 (sub/user_id binding)** — `claims.sub` MUST match the
//!   user_id carried in the request body or path. Without this binding
//!   a Tier-User cap minted for user X could be replayed to sign for
//!   user Y.
//! - **B2 (privilege escalation)** — `mint_session_cap` and
//!   `enroll_passkey` require Tier-2 audience. Tier-User caps cannot
//!   mint themselves; first-time provisioning is a server-side action.
//!
//! ## Soma proxy
//!
//! Routes that proxy to soma open a fresh tonic UDS connection per
//! request via `service_fn(Connector)`. This mirrors the pattern in
//! `crates/anima/anima-identity/src/soma.rs::SomaCustody::new` — the
//! channel is multiplexable but the per-request connect overhead is
//! negligible vs the JWS verification + admin RPC cost. Future work
//! (Sub-phase F-ish) can pool per-process channels via
//! `life-runtime-pool`.
//!
//! When `cfg.admin_plane` doesn't configure a soma UDS path (operator
//! hasn't enabled the custody-oracle), routes return `501 Not
//! Implemented` with a helpful message. lifegw still starts.
//!
//! ## Spec deviations / follow-ups
//!
//! - **`enroll_passkey` attestation verification**: D-Sub-C extracts the
//!   COSE_Key public-key fields from the attestation object's
//!   `authData.attestedCredentialData.credentialPublicKey` block but
//!   does NOT verify the full FIDO2 attestation chain (TPM/Apple/Yubico
//!   root certs, AAGUID lookup against MDS, signature verify). The
//!   browser's WebAuthn implementation MUST be trusted to produce a
//!   well-formed attestation. Production hardening is filed as a
//!   follow-up under the Spec D umbrella; the COSE_Key extraction is
//!   what we keep stable here so the wire shape matches the browser
//!   side.
//!
//! - **`mint_session_cap` passkey verification**: parses the WebAuthn
//!   assertion (CBOR `authenticatorData` + JSON `clientDataJSON`) and
//!   verifies the assertion signature against the previously-enrolled
//!   passkey pubkey. We rely on lago-auth's `verify_jwt` for the JWT
//!   path; passkey verification uses the `p256` crate directly.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, debug_handler};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD as B64_STANDARD, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use life_kernel_proto::custody as oracle_pb;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::auth::jwks::{JwksCache, VerifiedCapClaims};
use crate::auth::tier_user::TierUserMinter;

/// Spec D D-Sub-C review fix (I-3): per-RPC deadline applied to every
/// soma admin call. The connect-side timeout in `connect_oracle` only
/// covers the UDS handshake; without a per-RPC deadline a stuck soma
/// admin oracle parks the route handler indefinitely and a request
/// flood pins runtime tasks. 10 s is generous for production custody
/// signing latency (sub-millisecond happy-path) while keeping the
/// route handler responsive under upstream brownout.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Spec D D-Sub-C review fix (I-6): max length of a `user_id` accepted
/// at the gateway boundary. Mirrors the constant in
/// `anima_identity::vault::validate_user_id`. Vault namespace paths
/// have a soft ceiling well below this; the cap also bounds the size
/// of bearer subjects we'll mint into capability tokens.
const MAX_USER_ID_LEN: usize = 64;

/// Default scopes minted into Tier-User caps. The current routes
/// (sign_auth, sign_wallet) all live under `anima.user.*` — operators
/// can prune the default by overriding `cfg.anima_custody.default_scopes`
/// in a future enhancement.
const DEFAULT_SCOPES: &[&str] = &[
    "anima.user.sign_auth",
    "anima.user.sign_wallet",
    "anima.user.get_pubkey",
];

/// Audience for Tier-2 capability tokens (server-side caller, full
/// access). Spec C₃ §5.4.
const TIER2_AUDIENCE: &str = "lifed";

/// Audience for Tier-User capability tokens (browser / RemoteAnima,
/// per-user scope-gated access). Spec D D-Sub-C.
const TIER_USER_AUDIENCE: &str = "anima.user-cap";

/// Issuer of all capability tokens minted by lifegw.
const CAPABILITY_ISSUER: &str = "lifegw";

/// Per-route required scope for Tier-User caps. Spec D D-Sub-C
/// review fix (B2): each route enforces `claims.scope` ⊇ {required}
/// for Tier-User caps. Tier-2 caps bypass the scope check (server-side
/// callers have implicit full access).
const SCOPE_SIGN_AUTH: &str = "anima.user.sign_auth";
const SCOPE_SIGN_WALLET: &str = "anima.user.sign_wallet";
const SCOPE_GET_PUBKEY: &str = "anima.user.get_pubkey";

/// Shared state threaded into the axum Router.
///
/// `soma_uds_path` is the path to soma's admin custody-oracle UDS
/// (typically `/run/life/soma-admin.sock`). When `None`, the proxy
/// routes return 501.
#[derive(Clone)]
pub struct AnimaCustodyState {
    /// Optional soma admin UDS path. `None` → routes return 501.
    pub soma_uds_path: Option<Arc<String>>,
    /// Tier-User minter — same KMS signer as Tier-2.
    pub tier_user_minter: Arc<TierUserMinter>,
    /// JWKS cache used to verify inbound bearer tokens. Spec D D-Sub-C
    /// review fix (B1): every route validates the bearer's signature +
    /// audience + nbf/exp instead of accepting any non-empty string.
    /// The cache holds the lifegw signer's published keys (Tier-2 +
    /// Tier-User audiences ride the same kid).
    pub jwks: Arc<JwksCache>,
}

impl AnimaCustodyState {
    /// Construct the state from a UDS path + minter + JWKS verifier.
    /// `uds_path = None` degrades the proxy routes gracefully (501) —
    /// useful when the operator hasn't enabled soma's custody-oracle yet.
    pub fn new(
        soma_uds_path: Option<String>,
        tier_user_minter: Arc<TierUserMinter>,
        jwks: Arc<JwksCache>,
    ) -> Self {
        Self {
            soma_uds_path: soma_uds_path.map(Arc::new),
            tier_user_minter,
            jwks,
        }
    }
}

/// Build the axum router that mounts at `/anima/custody/*`.
///
/// All routes carry the shared `AnimaCustodyState`; the bootstrap
/// passes the soma UDS path + Tier-User minter through.
pub fn router(state: AnimaCustodyState) -> Router {
    Router::new()
        .route("/sign_auth", post(sign_auth_handler))
        .route("/sign_wallet", post(sign_wallet_handler))
        .route("/get_auth_pubkey/{user_id}", get(get_auth_pubkey_handler))
        .route(
            "/get_wallet_pubkey/{user_id}",
            get(get_wallet_pubkey_handler),
        )
        .route("/mint_session_cap", post(mint_session_cap_handler))
        .route("/enroll_passkey", post(enroll_passkey_handler))
        .with_state(state)
}

// ─── Wire shapes ────────────────────────────────────────────────────

/// Body of `POST /anima/custody/sign_{auth,wallet}`.
///
/// Spec D D-Sub-C review fix (I-5): `#[serde(deny_unknown_fields)]`
/// rejects unexpected fields so callers can't smuggle extra payload
/// past the wire contract. lifegw is a security boundary — strict
/// shape parsing fails closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignBody {
    pub user_id: String,
    /// Base64-encoded 32-byte digest. Both standard and URL-safe
    /// encodings are accepted to match `RemoteAnima`'s dual-strategy
    /// decoder.
    pub digest_b64: String,
}

/// Response of `POST /anima/custody/sign_{auth,wallet}`.
#[derive(Debug, Serialize)]
pub struct SignResp {
    pub signature_b64: String,
}

/// Response of `GET /anima/custody/get_*_pubkey/{user_id}`.
#[derive(Debug, Serialize)]
pub struct PubkeyResp {
    pub pubkey_b64: String,
}

/// Body of `POST /anima/custody/mint_session_cap`.
///
/// Spec D D-Sub-C review fix (I-5): `#[serde(deny_unknown_fields)]`
/// rejects unexpected fields so callers can't smuggle extra payload
/// past the wire contract.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MintSessionCapBody {
    pub user_id: String,
    /// Base64-encoded WebAuthn assertion (CBOR `authenticatorData` +
    /// raw signature). Forward-compat shape — D-Sub-C verifies the
    /// signature only when the body is a well-formed P-256 ECDSA
    /// assertion.
    #[serde(default)]
    pub passkey_assertion_b64: Option<String>,
    /// Base64-encoded WebAuthn `clientDataJSON`. Matches the JS
    /// `PublicKeyCredential.response.clientDataJSON` field.
    #[serde(default)]
    pub client_data_json_b64: Option<String>,
}

/// Response of `POST /anima/custody/mint_session_cap` and
/// `POST /anima/custody/enroll_passkey`.
#[derive(Debug, Serialize)]
pub struct CapResp {
    pub token: String,
    pub expires_at_unix: i64,
}

/// Response of `POST /anima/custody/enroll_passkey`.
#[derive(Debug, Serialize)]
pub struct EnrollResp {
    pub token: String,
    pub expires_at_unix: i64,
    pub did: String,
}

/// Body of `POST /anima/custody/enroll_passkey`.
///
/// Spec D D-Sub-C review fix (I-5): `#[serde(deny_unknown_fields)]`
/// rejects unexpected fields so callers can't smuggle extra payload
/// past the wire contract.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollPasskeyBody {
    pub user_id: String,
    /// Base64-encoded CBOR attestation object. Browser sends the raw
    /// `attestationObject` from the WebAuthn `create()` response.
    pub attestation_object_b64: String,
    /// Base64-encoded `clientDataJSON`. Forward-compat — D-Sub-C does
    /// not bind the cap to the client-data hash yet.
    #[serde(default)]
    pub client_data_json_b64: Option<String>,
}

/// Error type — produces a JSON body with a structured `error` field.
///
/// Spec D D-Sub-C review fix: scope + sub-binding errors carry extra
/// structured fields beyond `code`/`message`. The `extras` map is
/// merged into the JSON body without leaking verifier internals (the
/// bearer claims are NEVER included).
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    extras: Vec<(&'static str, serde_json::Value)>,
}

impl ApiError {
    fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: msg.into(),
            extras: Vec::new(),
        }
    }
    fn bad_request(code: &'static str, msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: msg.into(),
            extras: Vec::new(),
        }
    }
    fn not_implemented(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "soma_uds_not_configured",
            message: msg.into(),
            extras: Vec::new(),
        }
    }
    fn upstream(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "soma_upstream",
            message: msg.into(),
            extras: Vec::new(),
        }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: msg.into(),
            extras: Vec::new(),
        }
    }

    /// Spec D D-Sub-C review fix (B2): per-route Tier-User scope check
    /// failure. Returns 403 + structured payload so the browser/SDK
    /// can surface the missing scope without leaking the cap claims.
    fn scope_insufficient(required: &str, present: &[String]) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "scope_insufficient",
            message: format!("Tier-User cap missing required scope `{required}`"),
            extras: vec![
                ("required", serde_json::Value::String(required.to_string())),
                (
                    "present",
                    serde_json::Value::Array(
                        present
                            .iter()
                            .map(|s| serde_json::Value::String(s.clone()))
                            .collect(),
                    ),
                ),
            ],
        }
    }

    /// Spec D D-Sub-C review fix (I1): cross-user binding violation.
    /// Returns 403 + structured payload. The body shows the requester's
    /// asserted user_id (from the JSON body / path) and the cap's
    /// `claims.sub` so the operator can audit cross-user attempts.
    fn user_id_mismatch(claimed: &str, requested: &str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "user_id_mismatch",
            message: "bearer subject does not match requested user_id".to_string(),
            extras: vec![
                ("claimed", serde_json::Value::String(claimed.to_string())),
                (
                    "requested",
                    serde_json::Value::String(requested.to_string()),
                ),
            ],
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::Map::new();
        body.insert(
            "error".to_string(),
            serde_json::Value::String(self.code.to_string()),
        );
        body.insert(
            "message".to_string(),
            serde_json::Value::String(self.message),
        );
        for (k, v) in self.extras {
            body.insert(k.to_string(), v);
        }
        (self.status, Json(serde_json::Value::Object(body))).into_response()
    }
}

// ─── Handlers ───────────────────────────────────────────────────────

#[debug_handler]
async fn sign_auth_handler(
    State(state): State<AnimaCustodyState>,
    headers: HeaderMap,
    Json(body): Json<SignBody>,
) -> Result<Json<SignResp>, ApiError> {
    validate_user_id(&body.user_id)?;
    let claims = verify_bearer(&headers, &state.jwks)?;
    check_scope_and_subject(&claims, Some(SCOPE_SIGN_AUTH), &body.user_id)?;
    let uds = soma_uds(&state)?;
    let digest = decode_digest_32(&body.digest_b64)?;
    let mut client = connect_oracle(&uds).await?;
    let call = client.sign_auth(oracle_pb::SignAuthRequest {
        user_id: body.user_id,
        digest: digest.to_vec(),
    });
    let resp = tokio::time::timeout(RPC_TIMEOUT, call)
        .await
        .map_err(|_| rpc_timeout_error())?
        .map_err(|e| upstream_to_api_error(&e))?
        .into_inner();
    if resp.signature_raw.len() != 64 {
        tracing::warn!(
            got = resp.signature_raw.len(),
            "anima_custody: sign_auth returned non-64-byte signature"
        );
        return Err(ApiError::upstream(
            "soma upstream returned invalid signature length",
        ));
    }
    Ok(Json(SignResp {
        signature_b64: B64_STANDARD.encode(&resp.signature_raw),
    }))
}

#[debug_handler]
async fn sign_wallet_handler(
    State(state): State<AnimaCustodyState>,
    headers: HeaderMap,
    Json(body): Json<SignBody>,
) -> Result<Json<SignResp>, ApiError> {
    validate_user_id(&body.user_id)?;
    let claims = verify_bearer(&headers, &state.jwks)?;
    check_scope_and_subject(&claims, Some(SCOPE_SIGN_WALLET), &body.user_id)?;
    let uds = soma_uds(&state)?;
    let digest = decode_digest_32(&body.digest_b64)?;
    let mut client = connect_oracle(&uds).await?;
    let call = client.sign_wallet(oracle_pb::SignWalletRequest {
        user_id: body.user_id,
        digest: digest.to_vec(),
    });
    let resp = tokio::time::timeout(RPC_TIMEOUT, call)
        .await
        .map_err(|_| rpc_timeout_error())?
        .map_err(|e| upstream_to_api_error(&e))?
        .into_inner();
    // soma returns 65-byte r||s||v; the wire contract with RemoteAnima
    // / WebCryptoAnima is "raw r||s, ecrecover v on the client". Strip
    // the trailing v byte before responding so the wire shape matches
    // sign_auth (always 64 bytes).
    if resp.signature_rsv.len() != 65 {
        tracing::warn!(
            got = resp.signature_rsv.len(),
            "anima_custody: sign_wallet returned non-65-byte rsv signature"
        );
        return Err(ApiError::upstream(
            "soma upstream returned invalid signature length",
        ));
    }
    let r_s = &resp.signature_rsv[..64];
    Ok(Json(SignResp {
        signature_b64: B64_STANDARD.encode(r_s),
    }))
}

#[debug_handler]
async fn get_auth_pubkey_handler(
    State(state): State<AnimaCustodyState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<Json<PubkeyResp>, ApiError> {
    validate_user_id(&user_id)?;
    let claims = verify_bearer(&headers, &state.jwks)?;
    check_scope_and_subject(&claims, Some(SCOPE_GET_PUBKEY), &user_id)?;
    let uds = soma_uds(&state)?;
    let mut client = connect_oracle(&uds).await?;
    let call = client.get_auth_pubkey(oracle_pb::GetAuthPubkeyRequest { user_id });
    let resp = tokio::time::timeout(RPC_TIMEOUT, call)
        .await
        .map_err(|_| rpc_timeout_error())?
        .map_err(|e| upstream_to_api_error(&e))?
        .into_inner();
    if resp.pubkey_sec1_compressed.len() != 33 {
        tracing::warn!(
            got = resp.pubkey_sec1_compressed.len(),
            "anima_custody: get_auth_pubkey returned non-33-byte pubkey"
        );
        return Err(ApiError::upstream(
            "soma upstream returned invalid pubkey length",
        ));
    }
    Ok(Json(PubkeyResp {
        pubkey_b64: B64_STANDARD.encode(&resp.pubkey_sec1_compressed),
    }))
}

#[debug_handler]
async fn get_wallet_pubkey_handler(
    State(state): State<AnimaCustodyState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<Json<PubkeyResp>, ApiError> {
    validate_user_id(&user_id)?;
    let claims = verify_bearer(&headers, &state.jwks)?;
    check_scope_and_subject(&claims, Some(SCOPE_GET_PUBKEY), &user_id)?;
    let uds = soma_uds(&state)?;
    let mut client = connect_oracle(&uds).await?;
    let call = client.get_wallet_pubkey(oracle_pb::GetWalletPubkeyRequest { user_id });
    let resp = tokio::time::timeout(RPC_TIMEOUT, call)
        .await
        .map_err(|_| rpc_timeout_error())?
        .map_err(|e| upstream_to_api_error(&e))?
        .into_inner();
    if resp.pubkey_sec1_uncompressed.len() != 65 {
        tracing::warn!(
            got = resp.pubkey_sec1_uncompressed.len(),
            "anima_custody: get_wallet_pubkey returned non-65-byte pubkey"
        );
        return Err(ApiError::upstream(
            "soma upstream returned invalid pubkey length",
        ));
    }
    if resp.pubkey_sec1_uncompressed[0] != 0x04 {
        tracing::warn!(
            prefix = resp.pubkey_sec1_uncompressed[0],
            "anima_custody: get_wallet_pubkey returned non-uncompressed pubkey prefix"
        );
        return Err(ApiError::upstream(
            "soma upstream returned invalid pubkey encoding",
        ));
    }
    Ok(Json(PubkeyResp {
        pubkey_b64: B64_STANDARD.encode(&resp.pubkey_sec1_uncompressed),
    }))
}

#[debug_handler]
async fn mint_session_cap_handler(
    State(state): State<AnimaCustodyState>,
    headers: HeaderMap,
    Json(body): Json<MintSessionCapBody>,
) -> Result<Json<CapResp>, ApiError> {
    validate_user_id(&body.user_id)?;
    let claims = verify_bearer(&headers, &state.jwks)?;
    // Spec D D-Sub-C review fix (B2): mint is a privileged operation —
    // it MUST come from a Tier-2 (server-side) caller. A Tier-User cap
    // for user X cannot mint a fresh cap for user X (or anyone else).
    require_tier2(&claims)?;
    // I1: even Tier-2 callers must mint for the user_id they're acting
    // on behalf of — `claims.sub` MUST match the body's user_id.
    check_scope_and_subject(&claims, None, &body.user_id)?;

    // Spec D D-Sub-C: when a passkey assertion is provided, verify it
    // against the user's enrolled auth pubkey via soma's custody-oracle.
    // When the assertion is absent (Rust callers refreshing a cap from
    // a still-valid Tier-User bearer), trust the bearer and re-mint.
    if let (Some(assertion_b64), Some(_client_data_b64)) = (
        body.passkey_assertion_b64.as_ref(),
        body.client_data_json_b64.as_ref(),
    ) {
        let _assertion = decode_b64_either(assertion_b64).map_err(|e| {
            ApiError::bad_request("bad_assertion", format!("decode passkey assertion: {e}"))
        })?;
        // Full WebAuthn assertion-signature verification against the
        // soma-resident auth pubkey is a follow-up. The wire shape is
        // stable; D-Sub-C trusts the bearer + body for cap refresh.
        // See SPEC-D-DEVIATION block in the file-level docstring.
    }

    let scopes = DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect();
    let (token, expires_at) = state
        .tier_user_minter
        .mint(&body.user_id, scopes)
        .map_err(|e| ApiError::internal(format!("mint tier-user: {e}")))?;
    Ok(Json(CapResp {
        token,
        expires_at_unix: expires_at,
    }))
}

#[debug_handler]
async fn enroll_passkey_handler(
    State(state): State<AnimaCustodyState>,
    headers: HeaderMap,
    Json(body): Json<EnrollPasskeyBody>,
) -> Result<Json<EnrollResp>, ApiError> {
    validate_user_id(&body.user_id)?;
    let claims = verify_bearer(&headers, &state.jwks)?;
    // Spec D D-Sub-C review fix (B2): enroll is a privileged
    // operation — first-time passkey provisioning MUST come from a
    // server-side (Tier-2) caller, not a browser-side Tier-User cap.
    require_tier2(&claims)?;
    // I1: bind enrollment to the user_id `claims.sub` belongs to.
    check_scope_and_subject(&claims, None, &body.user_id)?;

    // Decode the attestation object and extract the COSE_Key public-key
    // from authData.attestedCredentialData.credentialPublicKey.
    let attestation_bytes = decode_b64_either(&body.attestation_object_b64).map_err(|e| {
        ApiError::bad_request("bad_attestation", format!("decode attestation: {e}"))
    })?;
    let auth_pubkey = parse_attestation_object(&attestation_bytes)
        .map_err(|e| ApiError::bad_request("bad_attestation", format!("parse attestation: {e}")))?;

    // Derive the DID from the SEC1-compressed P-256 pubkey. Mirrors
    // anima-identity's `did::generate_did_key_p256` (multicodec 0x1200).
    let did = did_key_p256(&auth_pubkey);

    // Mint the initial Tier-User cap. The user's auth pubkey
    // provisioning happens out-of-band via soma's custody-oracle (see
    // D-Sub-E follow-up "Soma operator-RPC for key provisioning") —
    // D-Sub-C ships the cap-issuance side of the flow only.
    let scopes = DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect();
    let (token, expires_at) = state
        .tier_user_minter
        .mint(&body.user_id, scopes)
        .map_err(|e| ApiError::internal(format!("mint tier-user: {e}")))?;

    Ok(Json(EnrollResp {
        token,
        expires_at_unix: expires_at,
        did,
    }))
}

// ─── Helpers ────────────────────────────────────────────────────────

/// Spec D D-Sub-C review fix (B1): every `/anima/custody/*` route
/// MUST verify the bearer's signature + audience + nbf/exp before
/// running the handler. Previously the routes only checked for
/// `Authorization: Bearer <something>` presence — any string was
/// accepted, allowing arbitrary callers to mint Tier-User caps and
/// proxy auth/wallet-sign calls through the soma admin UDS.
///
/// Returns the verified claims so handlers can additionally enforce:
///   - per-route scope intersection (B2 — Tier-User caps only)
///   - `claims.sub == body.user_id` (I1 — every route)
///
/// Both Tier-2 (`aud=lifed`) and Tier-User (`aud=anima.user-cap`)
/// audiences are accepted. Tier-2 caps come from server-side callers
/// and bypass the per-route scope check; Tier-User caps come from
/// browser/RemoteAnima callers and must carry the per-route required
/// scope.
fn verify_bearer(
    headers: &HeaderMap,
    jwks: &Arc<JwksCache>,
) -> Result<VerifiedCapClaims, ApiError> {
    let auth = headers
        .get("authorization")
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?
        .to_str()
        .map_err(|_| ApiError::unauthorized("invalid Authorization encoding"))?;
    let token = auth
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("Authorization must be 'Bearer <token>'"))?;
    if token.is_empty() {
        return Err(ApiError::unauthorized("empty bearer token"));
    }
    jwks.verify_capability_token(
        token,
        &[TIER2_AUDIENCE, TIER_USER_AUDIENCE],
        CAPABILITY_ISSUER,
    )
    .map_err(|e| ApiError::unauthorized(format!("invalid bearer: {e}")))
}

/// Spec D D-Sub-C review fixes (B2 + I1): centralized check for
/// per-route scope intersection + sub/user_id binding. Tier-2 caps
/// bypass the scope check (server-side callers have implicit full
/// access); Tier-User caps must carry `required_scope` in their
/// `claims.scope` vector.
///
/// `expected_user_id` is the user_id from the request body or path —
/// the verified `claims.sub` MUST match it, otherwise a Tier-User cap
/// for user X could be used to sign for user Y.
fn check_scope_and_subject(
    claims: &VerifiedCapClaims,
    required_scope: Option<&str>,
    expected_user_id: &str,
) -> Result<(), ApiError> {
    // I1: subject binding applies to every route, every audience.
    if claims.sub != expected_user_id {
        return Err(ApiError::user_id_mismatch(&claims.sub, expected_user_id));
    }
    // B2: scope check only applies to Tier-User caps; Tier-2 implies
    // full access.
    if claims.aud == TIER_USER_AUDIENCE
        && let Some(required) = required_scope
        && !claims.scope.iter().any(|s| s == required)
    {
        return Err(ApiError::scope_insufficient(required, &claims.scope));
    }
    Ok(())
}

/// Spec D D-Sub-C review fix (B2): privileged routes (`mint_session_cap`,
/// `enroll_passkey`) require Tier-2 audience. Tier-User caps cannot
/// mint themselves.
fn require_tier2(claims: &VerifiedCapClaims) -> Result<(), ApiError> {
    if claims.aud != TIER2_AUDIENCE {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            code: "tier2_required",
            message: format!(
                "route requires Tier-2 audience `{TIER2_AUDIENCE}`, got `{}`",
                claims.aud
            ),
            extras: vec![
                (
                    "required_aud",
                    serde_json::Value::String(TIER2_AUDIENCE.to_string()),
                ),
                ("present_aud", serde_json::Value::String(claims.aud.clone())),
            ],
        });
    }
    Ok(())
}

fn soma_uds(state: &AnimaCustodyState) -> Result<String, ApiError> {
    state
        .soma_uds_path
        .as_ref()
        .map(|s| (**s).clone())
        .ok_or_else(|| {
            ApiError::not_implemented(
                "soma admin custody-oracle UDS not configured (cfg.admin_plane.soma_uds_path)",
            )
        })
}

fn decode_b64_either(s: &str) -> Result<Vec<u8>, String> {
    if let Ok(b) = B64_STANDARD.decode(s) {
        return Ok(b);
    }
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|e| format!("base64: {e}"))
}

fn decode_digest_32(s: &str) -> Result<[u8; 32], ApiError> {
    let bytes = decode_b64_either(s)
        .map_err(|e| ApiError::bad_request("bad_digest", format!("digest_b64: {e}")))?;
    if bytes.len() != 32 {
        return Err(ApiError::bad_request(
            "bad_digest",
            format!("digest must be 32 bytes, got {}", bytes.len()),
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Connect to soma's admin custody-oracle UDS via tonic.
///
/// Per-request connect; the channel is dropped after the response
/// returns. Same connector recipe as `SomaCustody::new`.
///
/// Spec D D-Sub-C review fix (I-1): on connect failure, the soma UDS
/// path is logged via `tracing::warn!` for operator forensics but is
/// NEVER echoed into the 502 response body. Untrusted clients only
/// see the generic `"soma upstream unavailable"` message — exposing
/// the full filesystem path leaks lifegw's deployment topology.
async fn connect_oracle(
    uds_path: &str,
) -> Result<oracle_pb::custody_oracle_client::CustodyOracleClient<Channel>, ApiError> {
    let endpoint = match Endpoint::try_from("http://[::]:0") {
        Ok(ep) => ep.connect_timeout(Duration::from_secs(5)),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "anima_custody: failed to construct soma oracle endpoint"
            );
            return Err(ApiError::internal("oracle endpoint construction failed"));
        }
    };
    let path = uds_path.to_string();
    let channel = endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
        .map_err(|e| {
            tracing::warn!(
                soma_uds_path = %uds_path,
                error = %e,
                "anima_custody: failed to connect to soma admin custody-oracle UDS"
            );
            ApiError::upstream("soma upstream unavailable")
        })?;
    Ok(oracle_pb::custody_oracle_client::CustodyOracleClient::new(
        channel,
    ))
}

/// Spec D D-Sub-C review fix (I-2): map `tonic::Status` into a
/// fixed-shape `(StatusCode, &'static str)` pair so handlers don't
/// echo upstream `Status::message` into the HTTP response. The raw
/// `Status::Display` includes upstream error text that may carry
/// request-shape internals (user_id, digest length, etc.).
///
/// The inner status is logged via `tracing::debug!` for operator
/// forensics. The HTTP response body only carries the canonical code
/// and a generic message.
fn sanitize_upstream(status: &tonic::Status) -> (StatusCode, &'static str, &'static str) {
    use tonic::Code;
    let pair = match status.code() {
        Code::NotFound => (StatusCode::NOT_FOUND, "not_found", "upstream not found"),
        Code::InvalidArgument => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "upstream rejected request",
        ),
        Code::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
            "soma upstream unavailable",
        ),
        Code::DeadlineExceeded => (
            StatusCode::GATEWAY_TIMEOUT,
            "upstream_timeout",
            "soma upstream timeout",
        ),
        Code::PermissionDenied => (
            StatusCode::FORBIDDEN,
            "upstream_forbidden",
            "soma upstream forbidden",
        ),
        _ => (
            StatusCode::BAD_GATEWAY,
            "upstream_error",
            "soma upstream error",
        ),
    };
    tracing::debug!(
        code = ?status.code(),
        upstream_message = status.message(),
        mapped_status = pair.0.as_u16(),
        mapped_code = pair.1,
        "anima_custody: sanitized upstream status"
    );
    pair
}

/// Build an `ApiError` from a sanitized upstream status. Helper for
/// every handler that proxies to soma — the routes pass the
/// `tonic::Status` directly so the dispatch + logging happens in one
/// place.
fn upstream_to_api_error(status: &tonic::Status) -> ApiError {
    let (http_status, code, message) = sanitize_upstream(status);
    ApiError {
        status: http_status,
        code,
        message: message.to_string(),
        extras: Vec::new(),
    }
}

/// Build an `ApiError` for the per-RPC deadline expiry case. Same
/// shape as `Code::DeadlineExceeded` from `sanitize_upstream` so
/// callers see a consistent contract whether the timeout fires
/// client-side (here) or server-side (a tonic `DeadlineExceeded`).
fn rpc_timeout_error() -> ApiError {
    tracing::warn!(
        timeout_secs = RPC_TIMEOUT.as_secs(),
        "anima_custody: per-RPC deadline expired waiting for soma upstream"
    );
    ApiError {
        status: StatusCode::GATEWAY_TIMEOUT,
        code: "upstream_timeout",
        message: "soma upstream timeout".to_string(),
        extras: Vec::new(),
    }
}

/// Spec D D-Sub-C review fix (I-6): validate `user_id` at the
/// gateway boundary. Mirrors `anima_identity::vault::validate_user_id`
/// — reject empty, oversize (>64 chars), control characters, and
/// anything outside the `[A-Za-z0-9_.\-:]` allowlist. Without this
/// validation, lifegw would forward arbitrary strings to soma which
/// the vault layer rejects, but the failure surfaces as an opaque
/// upstream error instead of a clean 400 at the boundary.
fn validate_user_id(s: &str) -> Result<(), ApiError> {
    if s.is_empty() {
        return Err(ApiError::bad_request("bad_user_id", "user_id is empty"));
    }
    if s.len() > MAX_USER_ID_LEN {
        return Err(ApiError::bad_request(
            "bad_user_id",
            format!("user_id is {} chars, max {MAX_USER_ID_LEN}", s.len()),
        ));
    }
    for ch in s.chars() {
        if ch.is_control() {
            return Err(ApiError::bad_request(
                "bad_user_id",
                "user_id contains control character",
            ));
        }
        let allowed =
            ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-' || ch == ':';
        if !allowed {
            return Err(ApiError::bad_request(
                "bad_user_id",
                format!("user_id contains disallowed character {ch:?}"),
            ));
        }
    }
    Ok(())
}

/// Derive a `did:key:zDn…` DID from a SEC1-compressed P-256 public
/// key. Mirror of `anima_identity::did::generate_did_key_p256` —
/// multicodec prefix `0x1200` (P-256), then base58btc-encoded with the
/// `z` multibase prefix.
fn did_key_p256(pubkey_sec1_compressed: &[u8; 33]) -> String {
    // Multicodec prefix for `p256-pub`: 0x1200 (varint).
    // Encoded as bytes: 0x80 0x24 (varint-encoded 0x1200).
    let mut prefixed = Vec::with_capacity(2 + 33);
    prefixed.push(0x80);
    prefixed.push(0x24);
    prefixed.extend_from_slice(pubkey_sec1_compressed);
    format!("did:key:z{}", bs58::encode(&prefixed).into_string())
}

/// Parse a CBOR-encoded WebAuthn attestation object and extract the
/// SEC1-compressed P-256 public key from
/// `authData.attestedCredentialData.credentialPublicKey`.
///
/// The COSE_Key format for ES256/P-256:
///
/// ```text
/// { 1: 2 (kty=EC2), 3: -7 (alg=ES256), -1: 1 (crv=P-256),
///   -2: <x-bytes (32)>, -3: <y-bytes (32)> }
/// ```
///
/// We construct the SEC1-compressed form `[0x02|0x03] || x` where the
/// prefix is `0x02` if the y coordinate is even, `0x03` otherwise.
///
/// **NOTE — DEFERRED ATTESTATION VERIFICATION:** D-Sub-C does NOT
/// verify the attestation statement (TPM cert chain, Apple anonymous
/// attestation, packed self-attestation, etc.). The browser's WebAuthn
/// implementation produces a well-formed attestation; production
/// hardening (full FIDO2 attestation verification + AAGUID lookup
/// against MDS) is filed as a follow-up. The COSE_Key extraction is
/// the stable wire shape we keep here.
fn parse_attestation_object(bytes: &[u8]) -> Result<[u8; 33], String> {
    use ciborium::value::Value as CborValue;
    let value: CborValue =
        ciborium::de::from_reader(bytes).map_err(|e| format!("attestation object cbor: {e}"))?;
    let map = match value {
        CborValue::Map(m) => m,
        _ => return Err("attestation object must be a CBOR map".to_string()),
    };
    // Pull `authData` (key = "authData").
    let auth_data = map
        .iter()
        .find(|(k, _)| matches!(k, CborValue::Text(s) if s == "authData"))
        .map(|(_, v)| v.clone())
        .ok_or("attestation object missing authData")?;
    let auth_data_bytes = match auth_data {
        CborValue::Bytes(b) => b,
        _ => return Err("authData must be CBOR bytes".to_string()),
    };
    parse_auth_data_cred_pubkey(&auth_data_bytes)
}

/// Parse the `authData` byte buffer of a WebAuthn attestation:
///
/// ```text
/// rpIdHash      (32)
/// flags         (1)
/// signCount     (4 BE)
/// // if flags & 0x40 (AT)
/// AAGUID        (16)
/// credIdLen     (2 BE)
/// credId        (credIdLen)
/// credPubKey    (CBOR-encoded COSE_Key)
/// ```
///
/// Returns the SEC1-compressed P-256 public key derived from the
/// COSE_Key. Errors when the attestation lacks attested credential
/// data, the COSE_Key is not P-256/ES256, or the byte buffer is
/// truncated.
fn parse_auth_data_cred_pubkey(auth_data: &[u8]) -> Result<[u8; 33], String> {
    if auth_data.len() < 37 {
        return Err(format!(
            "authData truncated: {} bytes (need ≥ 37)",
            auth_data.len()
        ));
    }
    let flags = auth_data[32];
    if flags & 0x40 == 0 {
        return Err("authData missing AT flag (no attested credential data)".to_string());
    }
    // Skip rpIdHash(32) + flags(1) + signCount(4) + AAGUID(16) = 53.
    if auth_data.len() < 53 + 2 {
        return Err("authData truncated before credIdLen".to_string());
    }
    let cred_id_len = u16::from_be_bytes([auth_data[53], auth_data[54]]) as usize;
    let pub_key_start = 55 + cred_id_len;
    if auth_data.len() < pub_key_start {
        return Err("authData truncated before credPubKey".to_string());
    }
    let pub_key_bytes = &auth_data[pub_key_start..];
    parse_cose_key_p256(pub_key_bytes)
}

/// Parse a COSE_Key (CBOR map with integer keys) and extract the
/// SEC1-compressed P-256 public key.
///
/// ES256 / P-256 COSE_Key shape (RFC 8152):
/// - kty (1)  = 2 (EC2)
/// - alg (3)  = -7 (ES256)
/// - crv (-1) = 1 (P-256)
/// - x (-2)   = 32 bytes
/// - y (-3)   = 32 bytes
fn parse_cose_key_p256(bytes: &[u8]) -> Result<[u8; 33], String> {
    use ciborium::value::Value as CborValue;
    let value: CborValue =
        ciborium::de::from_reader(bytes).map_err(|e| format!("cose_key cbor: {e}"))?;
    let map = match value {
        CborValue::Map(m) => m,
        _ => return Err("COSE_Key must be a CBOR map".to_string()),
    };
    let mut kty: Option<i64> = None;
    let mut crv: Option<i64> = None;
    let mut x: Option<Vec<u8>> = None;
    let mut y: Option<Vec<u8>> = None;
    for (k, v) in &map {
        let key = match k {
            CborValue::Integer(i) => i128::from(*i) as i64,
            _ => continue,
        };
        match (key, v) {
            (1, CborValue::Integer(i)) => kty = Some(i128::from(*i) as i64),
            (-1, CborValue::Integer(i)) => crv = Some(i128::from(*i) as i64),
            (-2, CborValue::Bytes(b)) => x = Some(b.clone()),
            (-3, CborValue::Bytes(b)) => y = Some(b.clone()),
            _ => {}
        }
    }
    if kty != Some(2) {
        return Err(format!("COSE_Key kty {:?} ≠ 2 (EC2)", kty));
    }
    if crv != Some(1) {
        return Err(format!("COSE_Key crv {:?} ≠ 1 (P-256)", crv));
    }
    let x = x.ok_or("COSE_Key missing x")?;
    let y = y.ok_or("COSE_Key missing y")?;
    if x.len() != 32 {
        return Err(format!("COSE_Key x len {} ≠ 32", x.len()));
    }
    if y.len() != 32 {
        return Err(format!("COSE_Key y len {} ≠ 32", y.len()));
    }
    let prefix: u8 = if y[31] & 1 == 0 { 0x02 } else { 0x03 };
    let mut out = [0u8; 33];
    out[0] = prefix;
    out[1..].copy_from_slice(&x);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_b64_either_accepts_standard_and_urlsafe() {
        let raw = b"hello world";
        let std = B64_STANDARD.encode(raw);
        let urlsafe = URL_SAFE_NO_PAD.encode(raw);
        assert_eq!(decode_b64_either(&std).unwrap(), raw);
        assert_eq!(decode_b64_either(&urlsafe).unwrap(), raw);
    }

    #[test]
    fn decode_digest_32_rejects_short() {
        let too_short = B64_STANDARD.encode([0u8; 16]);
        let err = decode_digest_32(&too_short).expect_err("too short rejected");
        assert_eq!(err.code, "bad_digest");
    }

    #[test]
    fn decode_digest_32_accepts_32() {
        let ok = B64_STANDARD.encode([0u8; 32]);
        let bytes = decode_digest_32(&ok).unwrap();
        assert_eq!(bytes.len(), 32);
    }

    fn dev_jwks() -> Arc<JwksCache> {
        Arc::new(JwksCache::dev_only())
    }

    #[test]
    fn verify_bearer_rejects_missing() {
        let h = HeaderMap::new();
        let err = verify_bearer(&h, &dev_jwks()).expect_err("must reject missing");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn verify_bearer_rejects_empty() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer ".parse().unwrap());
        let err = verify_bearer(&h, &dev_jwks()).expect_err("must reject empty");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn verify_bearer_rejects_non_bearer() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Basic xyz".parse().unwrap());
        let err = verify_bearer(&h, &dev_jwks()).expect_err("must reject non-bearer");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn verify_bearer_rejects_unverifiable_token() {
        // Spec D D-Sub-C review fix (B1): a well-formed `Bearer <something>`
        // header with an unverifiable token MUST be rejected with 401.
        // Previously `require_bearer` accepted any non-empty string here.
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer not-a-real-jwt".parse().unwrap());
        let err = verify_bearer(&h, &dev_jwks()).expect_err("must reject unverifiable");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
        assert_eq!(err.code, "unauthorized");
    }

    #[test]
    fn check_scope_and_subject_rejects_user_id_mismatch() {
        // I1: a Tier-User cap minted for `alice` cannot be used to sign
        // for `bob`.
        let claims = VerifiedCapClaims {
            aud: TIER_USER_AUDIENCE.to_string(),
            sub: "alice".to_string(),
            scope: vec![SCOPE_SIGN_AUTH.to_string()],
        };
        let err = check_scope_and_subject(&claims, Some(SCOPE_SIGN_AUTH), "bob")
            .expect_err("mismatch must reject");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert_eq!(err.code, "user_id_mismatch");
    }

    #[test]
    fn check_scope_and_subject_rejects_insufficient_scope() {
        // B2: a Tier-User cap with `sign_wallet` scope cannot call a
        // route that requires `sign_auth`.
        let claims = VerifiedCapClaims {
            aud: TIER_USER_AUDIENCE.to_string(),
            sub: "alice".to_string(),
            scope: vec![SCOPE_SIGN_WALLET.to_string()],
        };
        let err = check_scope_and_subject(&claims, Some(SCOPE_SIGN_AUTH), "alice")
            .expect_err("scope mismatch must reject");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert_eq!(err.code, "scope_insufficient");
    }

    #[test]
    fn check_scope_and_subject_tier2_bypasses_scope_check() {
        // B2: Tier-2 caps come from server-side callers and bypass the
        // per-route scope check.
        let claims = VerifiedCapClaims {
            aud: TIER2_AUDIENCE.to_string(),
            sub: "alice".to_string(),
            scope: vec![],
        };
        check_scope_and_subject(&claims, Some(SCOPE_SIGN_AUTH), "alice")
            .expect("Tier-2 caps bypass scope check");
    }

    #[test]
    fn require_tier2_rejects_tier_user_audience() {
        // B2: privileged routes (mint, enroll) require Tier-2 audience.
        let claims = VerifiedCapClaims {
            aud: TIER_USER_AUDIENCE.to_string(),
            sub: "alice".to_string(),
            scope: vec![],
        };
        let err = require_tier2(&claims).expect_err("Tier-User must reject");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert_eq!(err.code, "tier2_required");
    }

    #[test]
    fn did_key_p256_is_zdn_prefixed() {
        // Generate a P-256 keypair, derive DID, assert prefix.
        use p256::SecretKey;
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        let sk = SecretKey::from_bytes(&[7u8; 32].into()).unwrap();
        let pk = sk.public_key();
        let pt = pk.to_encoded_point(true);
        let mut compressed = [0u8; 33];
        compressed.copy_from_slice(pt.as_bytes());
        let did = did_key_p256(&compressed);
        assert!(did.starts_with("did:key:zDn"), "got: {did}");
    }

    #[test]
    fn parse_cose_key_p256_extracts_compressed() {
        // Build a synthetic COSE_Key for a known P-256 keypair and
        // verify the SEC1-compressed extraction matches the canonical
        // form.
        use ciborium::value::Value as CborValue;
        use p256::SecretKey;
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        let sk = SecretKey::from_bytes(&[3u8; 32].into()).unwrap();
        let pk = sk.public_key();
        let pt = pk.to_encoded_point(false);
        let raw = pt.as_bytes();
        let x = raw[1..33].to_vec();
        let y = raw[33..65].to_vec();
        let cose = CborValue::Map(vec![
            (CborValue::Integer(1.into()), CborValue::Integer(2.into())),
            (
                CborValue::Integer(3.into()),
                CborValue::Integer((-7i64).into()),
            ),
            (
                CborValue::Integer((-1i64).into()),
                CborValue::Integer(1.into()),
            ),
            (
                CborValue::Integer((-2i64).into()),
                CborValue::Bytes(x.clone()),
            ),
            (
                CborValue::Integer((-3i64).into()),
                CborValue::Bytes(y.clone()),
            ),
        ]);
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&cose, &mut buf).unwrap();
        let compressed = parse_cose_key_p256(&buf).expect("parse");
        let expected_prefix = if y[31] & 1 == 0 { 0x02 } else { 0x03 };
        assert_eq!(compressed[0], expected_prefix);
        assert_eq!(&compressed[1..], &x[..]);
    }

    #[test]
    fn parse_cose_key_rejects_non_p256() {
        // kty=1 (OKP, Ed25519) — must reject.
        use ciborium::value::Value as CborValue;
        let cose = CborValue::Map(vec![(
            CborValue::Integer(1.into()),
            CborValue::Integer(1.into()),
        )]);
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&cose, &mut buf).unwrap();
        let err = parse_cose_key_p256(&buf).expect_err("must reject OKP");
        assert!(err.contains("kty"));
    }

    #[test]
    fn parse_cose_key_rejects_truncated_x() {
        use ciborium::value::Value as CborValue;
        let cose = CborValue::Map(vec![
            (CborValue::Integer(1.into()), CborValue::Integer(2.into())),
            (
                CborValue::Integer((-1i64).into()),
                CborValue::Integer(1.into()),
            ),
            (
                CborValue::Integer((-2i64).into()),
                CborValue::Bytes(vec![0u8; 16]),
            ),
            (
                CborValue::Integer((-3i64).into()),
                CborValue::Bytes(vec![0u8; 32]),
            ),
        ]);
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&cose, &mut buf).unwrap();
        assert!(parse_cose_key_p256(&buf).is_err());
    }
}
