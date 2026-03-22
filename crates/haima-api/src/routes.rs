//! API route handlers.
//!
//! - `GET /health` — public (no auth, used by Railway health checks)
//! - `GET /state` — protected (requires JWT when auth is enabled)
//! - `POST /v1/facilitate` — x402 payment facilitation (public, credit-gated)
//! - `GET /v1/facilitator/stats` — facilitator dashboard stats (public)
//! - `GET /v1/credit/:agent_id` — get agent credit score and tier (public)
//! - `POST /v1/credit/:agent_id/check` — check if agent can spend amount (public)

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::{get, post};
use haima_core::credit::{CreditFactors, check_credit, compute_credit_score};
use haima_x402::{FacilitateRequest, verify_payment_header};
use serde::Deserialize;
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
        .route("/v1/credit/{agent_id}", get(get_credit_score))
        .route("/v1/credit/{agent_id}/check", post(check_credit_endpoint))
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
///
/// If `agent_id` is provided in the request body, the facilitator checks the
/// agent's credit score before processing. Insufficient credit results in rejection.
async fn facilitate(
    State(state): State<AppState>,
    Json(request): Json<FacilitateRequest>,
) -> Json<Value> {
    // If an agent_id is provided, check credit before proceeding.
    if let Some(ref agent_id) = request.agent_id {
        let scores = state.credit_scores.read().await;
        if let Some(credit) = scores.get(agent_id) {
            let result = check_credit(credit, request.amount_micro_usd);
            if !result.approved {
                return Json(json!({
                    "status": "rejected",
                    "reason": "insufficient_credit",
                    "details": format!(
                        "agent {} has tier '{}' with remaining limit {}",
                        agent_id, result.tier, result.remaining_limit
                    )
                }));
            }
        }
        // If no credit score cached for this agent, allow the transaction
        // (the facilitate endpoint also serves agents without credit profiles).
    }

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

/// `GET /v1/credit/:agent_id` — Return the credit score and tier for an agent.
///
/// If no credit score is cached for the agent, a default score is computed from
/// empty factors (tier = None, score = 0.0).
async fn get_credit_score(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Json<Value> {
    let scores = state.credit_scores.read().await;
    let credit = scores
        .get(&agent_id)
        .cloned()
        .unwrap_or_else(|| compute_credit_score(&agent_id, &CreditFactors::default()));
    Json(serde_json::to_value(&credit).unwrap_or_else(|_| json!({})))
}

/// Request body for `POST /v1/credit/:agent_id/check`.
#[derive(Debug, Deserialize)]
struct CreditCheckRequest {
    /// Amount to check in micro-USD.
    amount_micro_usd: u64,
}

/// `POST /v1/credit/:agent_id/check` — Check whether an agent can spend a given amount.
async fn check_credit_endpoint(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(request): Json<CreditCheckRequest>,
) -> Json<Value> {
    let scores = state.credit_scores.read().await;
    let credit = scores
        .get(&agent_id)
        .cloned()
        .unwrap_or_else(|| compute_credit_score(&agent_id, &CreditFactors::default()));
    let result = check_credit(&credit, request.amount_micro_usd);
    Json(serde_json::to_value(&result).unwrap_or_else(|_| json!({})))
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

    // --- Credit score endpoint ---

    #[tokio::test]
    async fn credit_score_unknown_agent_returns_default() {
        let state = AppState::default();
        let app = routes(state);

        let req = Request::builder()
            .uri("/v1/credit/agent-unknown")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["agent_id"], "agent-unknown");
        assert_eq!(json["tier"], "none");
        assert_eq!(json["score"], 0.0);
        assert_eq!(json["spending_limit_micro_usd"], 0);
    }

    #[tokio::test]
    async fn credit_score_cached_agent_returns_cached() {
        use haima_core::credit::{CreditFactors, CreditTier};

        let state = AppState::default();
        // Insert a cached credit score
        {
            let mut scores = state.credit_scores.write().await;
            let factors = CreditFactors {
                trust_score: 0.8,
                payment_history: 0.9,
                transaction_volume: 1_000_000,
                account_age_days: 60,
                economic_stability: 0.7,
            };
            let credit = compute_credit_score("agent-cached", &factors);
            scores.insert("agent-cached".to_string(), credit);
        }
        let app = routes(state);

        let req = Request::builder()
            .uri("/v1/credit/agent-cached")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["agent_id"], "agent-cached");
        // With those factors, agent should have at least Standard tier
        let tier: CreditTier = serde_json::from_value(json["tier"].clone()).unwrap();
        assert!(tier == CreditTier::Standard || tier == CreditTier::Premium);
    }

    // --- Credit check endpoint ---

    #[tokio::test]
    async fn credit_check_unknown_agent_always_rejected() {
        let state = AppState::default();
        let app = routes(state);

        let body = serde_json::json!({ "amount_micro_usd": 100 });
        let req = Request::builder()
            .method("POST")
            .uri("/v1/credit/agent-unknown/check")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        // Unknown agent has tier=none, limit=0 so any amount is rejected
        assert_eq!(json["approved"], false);
        assert_eq!(json["tier"], "none");
    }

    #[tokio::test]
    async fn credit_check_cached_agent_approved() {
        use haima_core::credit::CreditFactors;

        let state = AppState::default();
        {
            let mut scores = state.credit_scores.write().await;
            let factors = CreditFactors {
                trust_score: 1.0,
                payment_history: 1.0,
                transaction_volume: 10_000_000,
                account_age_days: 90,
                economic_stability: 1.0,
            };
            let credit = compute_credit_score("agent-premium", &factors);
            scores.insert("agent-premium".to_string(), credit);
        }
        let app = routes(state);

        let body = serde_json::json!({ "amount_micro_usd": 5000 });
        let req = Request::builder()
            .method("POST")
            .uri("/v1/credit/agent-premium/check")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["approved"], true);
        assert_eq!(json["tier"], "premium");
        assert_eq!(json["remaining_limit"], 10_000_000 - 5000);
    }

    // --- Credit-gated facilitate ---

    #[tokio::test]
    async fn facilitate_with_insufficient_credit_rejected() {
        use haima_core::credit::CreditFactors;

        let state = AppState::default();
        // Insert a credit score with Micro tier (limit = 1000)
        {
            let mut scores = state.credit_scores.write().await;
            let factors = CreditFactors {
                trust_score: 0.4,
                payment_history: 0.5,
                transaction_volume: 10_000,
                account_age_days: 7,
                economic_stability: 0.3,
            };
            let credit = compute_credit_score("agent-low", &factors);
            scores.insert("agent-low".to_string(), credit);
        }
        let app = routes(state);

        let body = serde_json::json!({
            "payment_header": make_valid_payment_header(),
            "resource_url": "https://api.example.com/data",
            "amount_micro_usd": 5000,
            "agent_id": "agent-low"
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
        assert_eq!(json["reason"], "insufficient_credit");
    }

    #[tokio::test]
    async fn facilitate_with_sufficient_credit_passes_through() {
        use haima_core::credit::CreditFactors;

        let state = AppState::default();
        // Insert a credit score with Premium tier (limit = 10M)
        {
            let mut scores = state.credit_scores.write().await;
            let factors = CreditFactors {
                trust_score: 1.0,
                payment_history: 1.0,
                transaction_volume: 10_000_000,
                account_age_days: 90,
                economic_stability: 1.0,
            };
            let credit = compute_credit_score("agent-premium", &factors);
            scores.insert("agent-premium".to_string(), credit);
        }
        let app = routes(state);

        let body = serde_json::json!({
            "payment_header": make_valid_payment_header(),
            "resource_url": "https://api.example.com/data",
            "amount_micro_usd": 1000,
            "agent_id": "agent-premium"
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
        // Should pass through to normal facilitation (settled)
        assert_eq!(json["status"], "settled");
    }

    #[tokio::test]
    async fn facilitate_without_agent_id_skips_credit_check() {
        let state = AppState::default();
        let app = routes(state);

        // No agent_id in request — should proceed without credit check
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
    }
}
