//! `POST /haima/x402/pay` — bespoke JSON route to initiate an x402
//! payment from the user's Anima-custodied wallet (BRO-1354, slice 2 of
//! the BRO-1341 x402 epic).
//!
//! This is the design's named edge route
//! (`docs/superpowers/specs/2026-06-02-lifegw-x402-route-design.md`
//! §Design — Route contract). It is a thin JSON shim over the gRPC
//! `life.v1.Wallet.X402Pay` method: the broomva.tech edge proxy
//! (`/api/x402/pay`, slice P2) mirrors the `/anima/custody/*` JSON
//! pattern, so the gateway exposes a JSON surface rather than requiring
//! the edge to speak gRPC-web.
//!
//! ## Flow (mirrors `anthropic_messages.rs` + `agent_http.rs`)
//!
//! ```text
//!   POST /haima/x402/pay  { resourceUrl, network?, maxAmountMicros? }
//!     Authorization: Bearer <Tier-1 JWS>
//!       1. verify Tier-1 (JwksCache::verify)
//!       2. enforce scope `x402:pay` for /life.v1.Wallet/X402Pay (the
//!          gateway is the scope-enforcement point — Spec C₃ §5.4)
//!       3. mint Tier-2 (Tier2Minter::mint, propagating scopes)
//!       4. dial lifed life.v1.Wallet.X402Pay over the upstream channel
//!       5. map the gRPC X402PayResp → JSON (resource bytes base64'd)
//! ```
//!
//! The `(user_id, project_id)` the payment is scoped to come from the
//! **verified Tier-1 token**, never the request body — a caller cannot
//! pay from another user's wallet.
//!
//! **base-sepolia only** in P1: lifed/haimad reject `network = "base"`
//! (mainnet) with `failed_precondition`, surfaced here as HTTP 412.
//!
//! Like `/anima/custody/*`, this route does its own bearer verification
//! because it sits OUTSIDE the tonic stack's `AuthLayer` (it is merged
//! into the axum router before the tonic fallback).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router, debug_handler};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64_STANDARD;
use serde::{Deserialize, Serialize};
use tonic::transport::Channel;

use life_runtime_proto::life::v1::{self as pb, wallet_client::WalletClient};

use crate::auth::jwks::JwksCache;
use crate::auth::scope::{self, ScopeError};
use crate::auth::tier2::Tier2Minter;

/// Per-RPC deadline for the upstream `life.v1.Wallet.X402Pay` call. The
/// substrate drives a full HTTP round-trip (GET → 402 → sign → retry →
/// settle), so this is more generous than a plain unary RPC.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// The gRPC route this bespoke HTTP alias maps to. Reused so scope
/// enforcement stays identical to the tonic-stack proxy path.
const X402_ROUTE: &str = "/life.v1.Wallet/X402Pay";

/// Shared state threaded into the axum router.
#[derive(Clone)]
pub struct HaimaX402State {
    /// Verifies inbound Tier-1 bearers.
    pub jwks: Arc<JwksCache>,
    /// Mints the Tier-2 capability attached to the upstream lifed call.
    pub minter: Arc<Tier2Minter>,
    /// Upstream tonic channel to lifed (the same handle the tonic stack
    /// proxies through).
    pub upstream: Channel,
}

/// Build the axum router mounted at `/haima/x402/*`.
pub fn router(state: HaimaX402State) -> Router {
    Router::new()
        .route("/pay", post(pay_handler))
        .with_state(state)
}

/// Body of `POST /haima/x402/pay`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PayBody {
    /// The x402-protected resource URL to fetch and pay for.
    pub resource_url: String,
    /// Payment network: `"base-sepolia"` (default) or `"base"` (mainnet,
    /// rejected until slice 3). Omitted → base-sepolia.
    #[serde(default)]
    pub network: Option<String>,
    /// Optional per-call ceiling in micro-credits, enforced before
    /// signing.
    #[serde(default)]
    pub max_amount_micros: Option<i64>,
}

/// Response of `POST /haima/x402/pay`. Flat mirror of the gRPC
/// `X402PayResp`; `status` is the discriminant.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayResp {
    /// `"settled"` | `"not_required"` | `"declined"`.
    pub status: String,
    pub tx_hash: String,
    pub network: String,
    pub recipient: String,
    pub micro_credits: i64,
    pub declined_reason: String,
    pub settled: bool,
    /// Base64-encoded fetched resource bytes (paid resource for
    /// `settled`, the unpaywalled body for `not_required`).
    pub resource_b64: String,
    pub resource_status: u32,
}

#[debug_handler]
async fn pay_handler(
    State(state): State<HaimaX402State>,
    headers: HeaderMap,
    Json(body): Json<PayBody>,
) -> Result<Json<PayResp>, ApiError> {
    if body.resource_url.is_empty() {
        return Err(ApiError::bad_request("empty resourceUrl"));
    }

    // 1. Verify the Tier-1 bearer.
    let tier1 = {
        let bearer = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .filter(|b| !b.is_empty())
            .ok_or_else(|| ApiError::unauthorized("missing Tier-1 bearer"))?;
        state
            .jwks
            .verify(bearer)
            .map_err(|e| ApiError::unauthorized(format!("invalid Tier-1: {e}")))?
    };

    // 2. Enforce the `x402:pay` scope (the gateway is the
    //    scope-enforcement point per Spec C₃ §5.4 — lifed validates the
    //    Tier-2 capability but does NOT re-check per-route scope).
    scope::enforce(X402_ROUTE, &tier1).map_err(|e| match e {
        ScopeError::Insufficient { .. } => ApiError::forbidden(e.to_string()),
        // The route is statically mapped — an UnknownRoute here is a
        // wiring bug, not a client error.
        ScopeError::UnknownRoute(_) => ApiError::internal("x402 route scope mapping missing"),
    })?;

    // 3. Mint a Tier-2 capability (propagates Tier-1 scopes + tier).
    let tier2 = state
        .minter
        .mint(&tier1)
        .map_err(|e| ApiError::internal(format!("mint tier-2: {e}")))?;

    // 4. Dial lifed's life.v1.Wallet.X402Pay. The (user, project) come
    //    from the verified Tier-1 token, never the request body.
    let mut client = WalletClient::new(state.upstream.clone());
    let mut req = tonic::Request::new(pb::X402PayReq {
        user_id: tier1.user_id.clone(),
        project_id: tier1.project_id.clone(),
        resource_url: body.resource_url,
        network: body.network.unwrap_or_default(),
        max_amount_micros: body.max_amount_micros,
    });
    attach_tier2(&mut req, &tier2)?;

    let resp = tokio::time::timeout(RPC_TIMEOUT, client.x402_pay(req))
        .await
        .map_err(|_| ApiError::gateway_timeout())?
        .map_err(|s| ApiError::from_status(&s))?
        .into_inner();

    Ok(Json(PayResp {
        status: resp.status,
        tx_hash: resp.tx_hash,
        network: resp.network,
        recipient: resp.recipient,
        micro_credits: resp.micro_credits,
        declined_reason: resp.declined_reason,
        settled: resp.settled,
        resource_b64: B64_STANDARD.encode(&resp.resource_body),
        resource_status: resp.resource_status,
    }))
}

/// Attach the minted Tier-2 capability to the upstream gRPC request.
fn attach_tier2<T>(req: &mut tonic::Request<T>, tier2: &str) -> Result<(), ApiError> {
    let value: tonic::metadata::MetadataValue<_> = format!("Bearer {tier2}")
        .parse()
        .map_err(|e| ApiError::internal(format!("encode tier-2 metadata: {e}")))?;
    req.metadata_mut().insert("authorization", value);
    Ok(())
}

/// Compact JSON error — `{ "error": <code>, "message": <msg> }`.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: msg.into(),
        }
    }
    fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "scope_insufficient",
            message: msg.into(),
        }
    }
    fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: msg.into(),
        }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: msg.into(),
        }
    }
    fn gateway_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            code: "upstream_timeout",
            message: "lifed x402 upstream timeout".to_string(),
        }
    }

    /// Map an upstream `tonic::Status` to an HTTP error. The substrate's
    /// `failed_precondition` (mainnet gate / over-balance) surfaces as
    /// 412 so the edge can distinguish a policy block from a 5xx. The
    /// upstream message is NOT echoed (it may carry request-shape
    /// internals); only the canonical code + a generic message ship.
    fn from_status(status: &tonic::Status) -> Self {
        use tonic::Code;
        let (http, code, message) = match status.code() {
            Code::InvalidArgument => (
                StatusCode::BAD_REQUEST,
                "bad_request",
                "invalid x402 request",
            ),
            Code::FailedPrecondition => (
                StatusCode::PRECONDITION_FAILED,
                "payment_precondition",
                "x402 payment precondition failed (network gate or balance)",
            ),
            Code::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "upstream rejected token",
            ),
            Code::PermissionDenied => (StatusCode::FORBIDDEN, "forbidden", "upstream denied"),
            Code::Unavailable => (
                StatusCode::BAD_GATEWAY,
                "upstream_unavailable",
                "x402 upstream unavailable",
            ),
            Code::DeadlineExceeded => (
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_timeout",
                "x402 upstream timeout",
            ),
            _ => (
                StatusCode::BAD_GATEWAY,
                "upstream_error",
                "x402 upstream error",
            ),
        };
        tracing::debug!(
            code = ?status.code(),
            upstream_message = status.message(),
            mapped = http.as_u16(),
            "haima_x402: sanitized upstream status"
        );
        Self {
            status: http,
            code,
            message: message.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code, "message": self.message });
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pay_body_deserializes_camel_case() {
        let json =
            r#"{"resourceUrl":"https://x.test/d","network":"base-sepolia","maxAmountMicros":500}"#;
        let body: PayBody = serde_json::from_str(json).expect("parse");
        assert_eq!(body.resource_url, "https://x.test/d");
        assert_eq!(body.network.as_deref(), Some("base-sepolia"));
        assert_eq!(body.max_amount_micros, Some(500));
    }

    #[test]
    fn pay_body_defaults_optional_fields() {
        let body: PayBody =
            serde_json::from_str(r#"{"resourceUrl":"https://x.test/d"}"#).expect("parse");
        assert!(body.network.is_none());
        assert!(body.max_amount_micros.is_none());
    }

    #[test]
    fn pay_body_rejects_unknown_fields() {
        // Strict shape parsing — lifegw is a security boundary.
        let err =
            serde_json::from_str::<PayBody>(r#"{"resourceUrl":"https://x.test/d","sneaky":true}"#);
        assert!(err.is_err(), "unknown field must be rejected");
    }

    #[test]
    fn pay_body_rejects_snake_case_resource_url() {
        // camelCase is the wire contract; snake_case must not parse.
        let err = serde_json::from_str::<PayBody>(r#"{"resource_url":"https://x.test/d"}"#);
        assert!(err.is_err(), "snake_case key must be rejected");
    }

    #[test]
    fn pay_resp_serializes_camel_case() {
        let resp = PayResp {
            status: "settled".to_string(),
            tx_hash: "0xabc".to_string(),
            network: "eip155:84532".to_string(),
            recipient: "0xrec".to_string(),
            micro_credits: 50,
            declined_reason: String::new(),
            settled: true,
            resource_b64: "Zm9v".to_string(),
            resource_status: 200,
        };
        let v = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(v["status"], "settled");
        assert_eq!(v["txHash"], "0xabc");
        assert_eq!(v["microCredits"], 50);
        assert_eq!(v["resourceB64"], "Zm9v");
        assert_eq!(v["resourceStatus"], 200);
    }

    #[test]
    fn from_status_maps_failed_precondition_to_412() {
        let err = ApiError::from_status(&tonic::Status::failed_precondition("mainnet gated"));
        assert_eq!(err.status, StatusCode::PRECONDITION_FAILED);
        assert_eq!(err.code, "payment_precondition");
        // Upstream message is NOT echoed.
        assert!(!err.message.contains("mainnet gated"));
    }

    #[test]
    fn from_status_maps_unavailable_to_502() {
        let err = ApiError::from_status(&tonic::Status::unavailable("haima down"));
        assert_eq!(err.status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn api_error_renders_json_body() {
        let resp = ApiError::bad_request("empty resourceUrl").into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
