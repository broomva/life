//! Outcome-based pricing API endpoints.
//!
//! - `POST /v1/outcomes/contracts` — register a task contract
//! - `GET  /v1/outcomes/contracts` — list all contracts
//! - `GET  /v1/outcomes/contracts/:contract_id` — get a specific contract
//! - `POST /v1/outcomes/:task_id/contract` — accept a task under a contract
//! - `POST /v1/outcomes/:task_id/verify` — verify task outcome manually + trigger billing
//! - `POST /v1/outcomes/:task_id/auto-verify` — run automated verifiers + trigger billing
//! - `POST /v1/outcomes/:task_id/refund` — process refund for a failed task
//! - `GET  /v1/outcomes/dashboard` — revenue dashboard
//! - `GET  /v1/outcomes/pending` — list pending tasks with SLA status

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::{get, post};
use chrono::Utc;
use haima_core::event::FinanceEventKind;
use haima_core::outcome::{
    CriterionResult, OutcomeVerification, RefundPolicy, SuccessCriterion, TaskComplexity,
    TaskContract, TaskOutcome, TaskType,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;

/// Build the outcome pricing routes.
pub fn outcome_routes(state: AppState) -> Router {
    Router::new()
        .route("/v1/outcomes/contracts", post(register_contract))
        .route("/v1/outcomes/contracts", get(list_contracts))
        .route(
            "/v1/outcomes/contracts/{contract_id}",
            get(get_contract),
        )
        .route(
            "/v1/outcomes/{task_id}/contract",
            post(accept_task),
        )
        .route(
            "/v1/outcomes/{task_id}/verify",
            post(verify_task),
        )
        .route(
            "/v1/outcomes/{task_id}/auto-verify",
            post(auto_verify_task),
        )
        .route(
            "/v1/outcomes/{task_id}/refund",
            post(refund_task),
        )
        .route("/v1/outcomes/dashboard", get(dashboard))
        .route("/v1/outcomes/pending", get(list_pending))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RegisterContractRequest {
    task_type: TaskType,
    name: String,
    price_floor_micro_credits: i64,
    price_ceiling_micro_credits: i64,
    #[serde(default)]
    success_criteria: Vec<SuccessCriterionInput>,
    #[serde(default)]
    refund_policy: Option<RefundPolicyInput>,
    #[serde(default = "default_min_trust")]
    min_trust_score: f64,
    #[serde(default)]
    custom_label: Option<String>,
}

fn default_min_trust() -> f64 {
    0.3
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SuccessCriterionInput {
    TestsPassed { scope: String },
    DataValidated { rule_id: String },
    ManualApproval { approver: String },
    WebhookConfirmed { url: String },
    Custom { description: String },
}

impl From<SuccessCriterionInput> for SuccessCriterion {
    fn from(input: SuccessCriterionInput) -> Self {
        match input {
            SuccessCriterionInput::TestsPassed { scope } => SuccessCriterion::TestsPassed { scope },
            SuccessCriterionInput::DataValidated { rule_id } => {
                SuccessCriterion::DataValidated { rule_id }
            }
            SuccessCriterionInput::ManualApproval { approver } => {
                SuccessCriterion::ManualApproval { approver }
            }
            SuccessCriterionInput::WebhookConfirmed { url } => {
                SuccessCriterion::WebhookConfirmed { url }
            }
            SuccessCriterionInput::Custom { description } => {
                SuccessCriterion::Custom { description }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct RefundPolicyInput {
    #[serde(default = "default_true")]
    auto_refund: bool,
    #[serde(default = "default_sla")]
    sla_seconds: u64,
    #[serde(default = "default_refund_pct")]
    refund_percentage: u8,
    #[serde(default = "default_grace")]
    grace_period_seconds: u64,
}

fn default_true() -> bool {
    true
}
fn default_sla() -> u64 {
    3600
}
fn default_refund_pct() -> u8 {
    100
}
fn default_grace() -> u64 {
    300
}

#[derive(Debug, Deserialize)]
struct AcceptTaskRequest {
    contract_id: String,
    agent_id: String,
    complexity: TaskComplexity,
    #[serde(default)]
    agent_trust_score: f64,
}

#[derive(Debug, Deserialize)]
struct VerifyTaskRequest {
    contract_id: String,
    results: Vec<CriterionResultInput>,
}

#[derive(Debug, Deserialize)]
struct CriterionResultInput {
    passed: bool,
    #[serde(default)]
    details: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AutoVerifyRequest {
    contract_id: String,
}

#[derive(Debug, Deserialize)]
struct RefundTaskRequest {
    contract_id: String,
    reason: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/outcomes/contracts` — Register a new task contract.
async fn register_contract(
    State(state): State<AppState>,
    Json(req): Json<RegisterContractRequest>,
) -> Json<Value> {
    if req.price_floor_micro_credits > req.price_ceiling_micro_credits {
        return Json(json!({
            "error": "price_floor cannot exceed price_ceiling"
        }));
    }
    if req.price_floor_micro_credits < 0 {
        return Json(json!({
            "error": "price_floor must be non-negative"
        }));
    }

    let contract_id = format!(
        "contract-{}-{}",
        req.task_type,
        Utc::now().timestamp_millis()
    );

    let refund_policy = req
        .refund_policy
        .map(|rp| RefundPolicy {
            auto_refund: rp.auto_refund,
            sla_seconds: rp.sla_seconds,
            refund_percentage: rp.refund_percentage,
            grace_period_seconds: rp.grace_period_seconds,
        })
        .unwrap_or_default();

    let contract = TaskContract {
        contract_id: contract_id.clone(),
        task_type: req.task_type,
        name: req.name,
        price_floor_micro_credits: req.price_floor_micro_credits,
        price_ceiling_micro_credits: req.price_ceiling_micro_credits,
        success_criteria: req.success_criteria.into_iter().map(Into::into).collect(),
        refund_policy,
        min_trust_score: req.min_trust_score.clamp(0.0, 1.0),
        custom_label: req.custom_label,
        created_at: Utc::now(),
    };

    let mut outcome_state = state.outcome_state.write().await;
    outcome_state.register_contract(contract.clone());

    Json(json!({
        "status": "registered",
        "contract_id": contract_id,
        "contract": serde_json::to_value(&contract).unwrap_or_default()
    }))
}

/// `GET /v1/outcomes/contracts` — List all registered contracts.
async fn list_contracts(State(state): State<AppState>) -> Json<Value> {
    let outcome_state = state.outcome_state.read().await;
    let contracts: Vec<_> = outcome_state.contracts.values().collect();
    Json(json!({
        "contracts": serde_json::to_value(&contracts).unwrap_or_default(),
        "count": contracts.len()
    }))
}

/// `GET /v1/outcomes/contracts/:contract_id` — Get a specific contract.
async fn get_contract(
    State(state): State<AppState>,
    Path(contract_id): Path<String>,
) -> Json<Value> {
    let outcome_state = state.outcome_state.read().await;
    match outcome_state.contracts.get(&contract_id) {
        Some(contract) => Json(serde_json::to_value(contract).unwrap_or_default()),
        None => Json(json!({"error": "contract not found"})),
    }
}

/// `POST /v1/outcomes/:task_id/contract` — Accept a task under a contract.
///
/// Resolves the price based on complexity + agent trust score, then records
/// a `TaskContracted` event.
async fn accept_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(req): Json<AcceptTaskRequest>,
) -> Json<Value> {
    let mut outcome_state = state.outcome_state.write().await;

    let contract = match outcome_state.contracts.get(&req.contract_id) {
        Some(c) => c.clone(),
        None => {
            return Json(json!({"error": "contract not found"}));
        }
    };

    // Check agent trust score meets minimum.
    if req.agent_trust_score < contract.min_trust_score {
        return Json(json!({
            "error": "agent_trust_score below contract minimum",
            "min_trust_score": contract.min_trust_score,
            "agent_trust_score": req.agent_trust_score
        }));
    }

    let price = contract.resolve_price(req.complexity, req.agent_trust_score);
    let sla_deadline_ms =
        Utc::now().timestamp_millis() + (contract.refund_policy.sla_seconds as i64 * 1000);

    let event = FinanceEventKind::TaskContracted {
        task_id: task_id.clone(),
        contract_id: req.contract_id.clone(),
        agent_id: req.agent_id.clone(),
        complexity: format!("{:?}", req.complexity).to_lowercase(),
        price_micro_credits: price,
        sla_deadline_ms,
    };

    outcome_state.apply(&event, Utc::now());

    Json(json!({
        "status": "contracted",
        "task_id": task_id,
        "contract_id": req.contract_id,
        "agent_id": req.agent_id,
        "complexity": req.complexity,
        "price_micro_credits": price,
        "sla_deadline_ms": sla_deadline_ms
    }))
}

/// `POST /v1/outcomes/:task_id/verify` — Verify a task's outcome.
///
/// Checks all criteria results, derives outcome, triggers billing on success.
async fn verify_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(req): Json<VerifyTaskRequest>,
) -> Json<Value> {
    let mut outcome_state = state.outcome_state.write().await;

    let contract = match outcome_state.contracts.get(&req.contract_id) {
        Some(c) => c.clone(),
        None => {
            return Json(json!({"error": "contract not found"}));
        }
    };

    // Find the pending task to get its price.
    let pending = outcome_state
        .pending_tasks
        .iter()
        .find(|t| t.task_id == task_id);
    let price = pending
        .map(|t| t.price_micro_credits)
        .unwrap_or(contract.price_floor_micro_credits);

    // Build criterion results.
    let criterion_results: Vec<CriterionResult> = req
        .results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let criterion = contract
                .success_criteria
                .get(i)
                .cloned()
                .unwrap_or(SuccessCriterion::Custom {
                    description: format!("criterion-{i}"),
                });
            CriterionResult {
                criterion,
                passed: r.passed,
                details: r.details.clone(),
                checked_at: Utc::now(),
            }
        })
        .collect();

    let outcome = OutcomeVerification::derive_outcome(&criterion_results);
    let criteria_passed = criterion_results.iter().filter(|r| r.passed).count() as u32;
    let criteria_total = criterion_results.len() as u32;

    let outcome_str = match outcome {
        TaskOutcome::Success => "success",
        TaskOutcome::Failure => "failure",
        TaskOutcome::PartialSuccess => "partial_success",
        TaskOutcome::Timeout => "timeout",
        TaskOutcome::Refunded => "refunded",
    };

    let event = FinanceEventKind::TaskVerified {
        task_id: task_id.clone(),
        contract_id: req.contract_id.clone(),
        outcome: outcome_str.to_string(),
        price_micro_credits: price,
        criteria_passed,
        criteria_total,
    };

    outcome_state.apply(&event, Utc::now());

    // On success, also generate a TaskBilled event for the FinancialState projection.
    let billing_triggered = outcome == TaskOutcome::Success || outcome == TaskOutcome::PartialSuccess;
    if billing_triggered {
        let bill_event = FinanceEventKind::TaskBilled {
            task_id: task_id.clone(),
            description: format!("outcome: {} ({})", contract.name, outcome_str),
            price_micro_credits: price,
            token: "USDC".into(),
            chain: "eip155:8453".into(),
        };
        let mut fs = state.financial_state.write().await;
        fs.apply(&bill_event, Utc::now());
    }

    let verification = OutcomeVerification {
        task_id: task_id.clone(),
        contract_id: req.contract_id,
        results: criterion_results,
        outcome,
        price_micro_credits: price,
        verified_at: Utc::now(),
    };

    Json(json!({
        "status": "verified",
        "verification": serde_json::to_value(&verification).unwrap_or_default(),
        "billing_triggered": billing_triggered
    }))
}

/// `POST /v1/outcomes/:task_id/auto-verify` — Run automated verifiers on a task.
///
/// Uses the `OutcomeEngine` to dispatch the appropriate verifier for each
/// success criterion in the contract. Triggers billing on success, refund on failure.
async fn auto_verify_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(req): Json<AutoVerifyRequest>,
) -> Json<Value> {
    let engine = haima_outcome::OutcomeEngine::new(
        state.outcome_state.clone(),
        state.financial_state.clone(),
    );

    match engine.verify_task(&task_id, &req.contract_id).await {
        Ok(result) => Json(json!({
            "status": "verified",
            "verification": serde_json::to_value(&result.verification).unwrap_or_default(),
            "billing_triggered": result.billing_triggered,
            "refund_triggered": result.refund_triggered,
            "refund_amount": result.refund_amount
        })),
        Err(e) => Json(json!({
            "error": e.to_string()
        })),
    }
}

/// `GET /v1/outcomes/pending` — List pending tasks with SLA status.
async fn list_pending(State(state): State<AppState>) -> Json<Value> {
    let outcome_state = state.outcome_state.read().await;
    let now_ms = Utc::now().timestamp_millis();

    let pending: Vec<Value> = outcome_state
        .pending_tasks
        .iter()
        .map(|t| {
            let remaining_ms = t.sla_deadline_ms - now_ms;
            let sla_status = if remaining_ms > 0 { "active" } else { "expired" };
            json!({
                "task_id": t.task_id,
                "contract_id": t.contract_id,
                "agent_id": t.agent_id,
                "price_micro_credits": t.price_micro_credits,
                "sla_deadline_ms": t.sla_deadline_ms,
                "remaining_ms": remaining_ms.max(0),
                "sla_status": sla_status,
                "contracted_at": t.contracted_at.to_rfc3339()
            })
        })
        .collect();

    Json(json!({
        "pending_tasks": pending,
        "count": pending.len()
    }))
}

/// `POST /v1/outcomes/:task_id/refund` — Process a refund for a failed task.
async fn refund_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(req): Json<RefundTaskRequest>,
) -> Json<Value> {
    let mut outcome_state = state.outcome_state.write().await;

    let contract = match outcome_state.contracts.get(&req.contract_id) {
        Some(c) => c.clone(),
        None => {
            return Json(json!({"error": "contract not found"}));
        }
    };

    // Look up the price from pending tasks or stats.
    let price = outcome_state
        .pending_tasks
        .iter()
        .find(|t| t.task_id == task_id)
        .map(|t| t.price_micro_credits)
        .unwrap_or(contract.price_floor_micro_credits);

    let refund_amount =
        (price as f64 * contract.refund_policy.refund_percentage as f64 / 100.0) as i64;

    let event = FinanceEventKind::TaskRefunded {
        task_id: task_id.clone(),
        contract_id: req.contract_id.clone(),
        refund_micro_credits: refund_amount,
        reason: req.reason.clone(),
    };

    outcome_state.apply(&event, Utc::now());

    Json(json!({
        "status": "refunded",
        "task_id": task_id,
        "contract_id": req.contract_id,
        "refund_micro_credits": refund_amount,
        "reason": req.reason
    }))
}

/// `GET /v1/outcomes/dashboard` — Revenue dashboard with per-task-type economics.
async fn dashboard(State(state): State<AppState>) -> Json<Value> {
    let outcome_state = state.outcome_state.read().await;
    let summary = outcome_state.dashboard();
    Json(serde_json::to_value(&summary).unwrap_or_else(|_| json!({})))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_app() -> Router {
        let state = AppState::default();
        outcome_routes(state)
    }

    #[tokio::test]
    async fn dashboard_empty() {
        let app = test_app();
        let req = Request::builder()
            .uri("/v1/outcomes/dashboard")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn list_contracts_empty() {
        let app = test_app();
        let req = Request::builder()
            .uri("/v1/outcomes/contracts")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
