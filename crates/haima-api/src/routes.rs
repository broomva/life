//! API route handlers.
//!
//! - `GET /health` — public (no auth, used by Railway health checks)
//! - `GET /state` — protected (requires JWT when auth is enabled)
//! - `POST /v1/facilitate` — x402 payment facilitation (public)
//! - `GET /v1/facilitator/stats` — facilitator dashboard stats (public)

use axum::Router;
use axum::extract::State;
use axum::response::Json;
use axum::routing::{get, post};
use haima_x402::{FacilitateRequest, verify_payment_header};
use serde_json::{Value, json};

use crate::AppState;
use crate::auth::require_auth;

pub fn routes(state: AppState) -> Router {
    // Protected routes — JWT required when auth is enabled
    let protected = Router::new()
        .route("/state", get(financial_state))
        .layer(axum::middleware::from_fn_with_state(
            state.auth_config.clone(),
            require_auth,
        ))
        .with_state(state.clone());

    // Public routes — no auth
    let public = Router::new()
        .route("/health", get(health))
        .route("/v1/facilitate", post(facilitate))
        .route("/v1/facilitator/stats", get(facilitator_stats))
        .with_state(state);

    // Merge: public routes are NOT behind the auth layer
    public.merge(protected)
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "haimad",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn financial_state(State(state): State<AppState>) -> Json<Value> {
    let fs = state.financial_state.read().await;
    Json(json!({
        "total_expenses": fs.total_expenses,
        "total_revenue": fs.total_revenue,
        "net_balance": fs.net_balance,
        "payment_count": fs.payment_count,
        "revenue_count": fs.revenue_count,
        "failed_count": fs.failed_count,
        "session_spend": fs.session_spend,
        "wallet_address": fs.wallet_address,
        "on_chain_balance": fs.on_chain_balance,
        "pending_bills": fs.pending_bills.len(),
    }))
}

/// `POST /v1/facilitate` — Verify an x402 payment header and return a settlement receipt.
async fn facilitate(
    State(state): State<AppState>,
    Json(request): Json<FacilitateRequest>,
) -> Json<Value> {
    let response = verify_payment_header(
        &request,
        state.facilitator_fee_bps,
        &state.facilitator_stats,
    );
    Json(serde_json::to_value(response).unwrap_or_else(|_| {
        json!({
            "status": "rejected",
            "reason": "internal_error",
            "details": "Failed to serialize response"
        })
    }))
}

/// `GET /v1/facilitator/stats` — Return facilitator dashboard statistics.
async fn facilitator_stats(State(state): State<AppState>) -> Json<Value> {
    let stats = state.facilitator_stats.snapshot();
    Json(serde_json::to_value(stats).unwrap_or_else(|_| json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthConfig;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use haima_x402::header::{PaymentSignatureHeader, encode_payment_signature};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use lago_auth::BroomvaClaims;
    use tower::ServiceExt;

    const TEST_SECRET: &str = "test-haima-secret-key";

    fn make_token(sub: &str, email: &str, secret: &str) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = BroomvaClaims {
            sub: sub.to_string(),
            email: email.to_string(),
            exp: now + 3600,
            iat: now,
        };
        let key = EncodingKey::from_secret(secret.as_bytes());
        encode(&Header::default(), &claims, &key).unwrap()
    }

    fn make_valid_payment_header() -> String {
        let sig = PaymentSignatureHeader {
            scheme: "exact".into(),
            network: "eip155:8453".into(),
            payload: hex::encode([0xabu8; 64]),
        };
        encode_payment_signature(&sig).unwrap()
    }

    // --- Health endpoint (always public) ---

    #[tokio::test]
    async fn health_without_token_returns_200() {
        let state = AppState::new(AuthConfig {
            jwt_secret: Some(TEST_SECRET.to_string()),
        });
        let app = routes(state);
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // --- State endpoint (protected) ---

    #[tokio::test]
    async fn state_without_token_auth_enabled_returns_401() {
        let state = AppState::new(AuthConfig {
            jwt_secret: Some(TEST_SECRET.to_string()),
        });
        let app = routes(state);
        let req = Request::builder()
            .uri("/state")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn state_with_valid_token_returns_200() {
        let state = AppState::new(AuthConfig {
            jwt_secret: Some(TEST_SECRET.to_string()),
        });
        let app = routes(state);
        let token = make_token("user1", "user1@broomva.tech", TEST_SECRET);
        let req = Request::builder()
            .uri("/state")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn state_with_wrong_secret_returns_401() {
        let state = AppState::new(AuthConfig {
            jwt_secret: Some(TEST_SECRET.to_string()),
        });
        let app = routes(state);
        let token = make_token("user1", "user1@broomva.tech", "wrong-secret");
        let req = Request::builder()
            .uri("/state")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // --- Auth disabled (local dev mode) ---

    #[tokio::test]
    async fn state_without_token_auth_disabled_returns_200() {
        let state = AppState::new(AuthConfig { jwt_secret: None });
        let app = routes(state);
        let req = Request::builder()
            .uri("/state")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // --- Facilitate endpoint (public) ---

    #[tokio::test]
    async fn facilitate_valid_payment_returns_settled() {
        let state = AppState::default();
        let app = routes(state);

        let body = serde_json::json!({
            "payment_header": make_valid_payment_header(),
            "resource_url": "https://api.example.com/data",
            "amount_micro_usd": 1000
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/facilitate")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "settled");
        assert!(json["receipt"].is_object());
        assert_eq!(json["receipt"]["amount_micro_usd"], 1000);
        assert_eq!(json["receipt"]["chain"], "base");
        assert_eq!(json["facilitator_fee_bps"], 15);
    }

    #[tokio::test]
    async fn facilitate_invalid_header_returns_rejected() {
        let state = AppState::default();
        let app = routes(state);

        let body = serde_json::json!({
            "payment_header": "not-valid-base64!!!",
            "resource_url": "https://api.example.com/data",
            "amount_micro_usd": 1000
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/facilitate")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "rejected");
        assert_eq!(json["reason"], "invalid_header");
    }

    #[tokio::test]
    async fn facilitate_updates_stats() {
        let state = AppState::default();
        let stats = state.facilitator_stats.clone();
        let app = routes(state);

        let body = serde_json::json!({
            "payment_header": make_valid_payment_header(),
            "resource_url": "https://api.example.com/data",
            "amount_micro_usd": 5000
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/facilitate")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let _resp = app.oneshot(req).await.unwrap();

        let snap = stats.snapshot();
        assert_eq!(snap.total_transactions, 1);
        assert_eq!(snap.total_volume_micro_usd, 5000);
    }

    // --- Facilitator stats endpoint (public) ---

    #[tokio::test]
    async fn facilitator_stats_returns_200() {
        let state = AppState::default();
        // Record some stats before querying
        state.facilitator_stats.record_settled(1_000_000, 1_500);
        state.facilitator_stats.record_rejected();
        let app = routes(state);

        let req = Request::builder()
            .uri("/v1/facilitator/stats")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total_transactions"], 1);
        assert_eq!(json["total_volume_micro_usd"], 1_000_000);
        assert_eq!(json["total_fees_micro_usd"], 1_500);
        assert_eq!(json["total_rejected"], 1);
    }
}
