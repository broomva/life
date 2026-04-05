//! Nous HTTP routes.

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use nous_core::EvalScore;
use serde::{Deserialize, Serialize};

use crate::store::ScoreStore;

/// Shared application state passed to handlers via axum `State`.
#[derive(Clone)]
pub struct AppState {
    pub store: ScoreStore,
    pub evaluator_count: u32,
}

/// Response for the eval endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct EvalResponse {
    pub session_id: String,
    pub scores: Vec<ScoreEntry>,
    pub aggregate_quality: f64,
}

/// A single score entry in the API response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ScoreEntry {
    pub evaluator: String,
    pub value: f64,
    pub label: String,
    pub layer: String,
    pub explanation: Option<String>,
}

/// Health check response.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub evaluator_count: u32,
}

/// Request body for score submission.
#[derive(Debug, Deserialize)]
pub struct SubmitScoresRequest {
    pub scores: Vec<EvalScore>,
}

/// Response after submitting scores.
#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitScoresResponse {
    pub accepted: usize,
}

/// Response listing sessions.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionsResponse {
    pub sessions: Vec<SessionSummary>,
}

/// Summary of a session's scores.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub score_count: usize,
    pub aggregate_quality: f64,
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        evaluator_count: state.evaluator_count,
    })
}

async fn get_eval(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<EvalResponse> {
    let scores = state.store.get_session_scores(&session_id);
    let aggregate_quality = state.store.aggregate_quality(&session_id);

    let score_entries = scores
        .into_iter()
        .map(|s| ScoreEntry {
            evaluator: s.evaluator,
            value: s.value,
            label: s.label.as_str().to_owned(),
            layer: s.layer.to_string(),
            explanation: s.explanation,
        })
        .collect();

    Json(EvalResponse {
        session_id,
        scores: score_entries,
        aggregate_quality,
    })
}

async fn post_scores(
    State(state): State<AppState>,
    Json(body): Json<SubmitScoresRequest>,
) -> (StatusCode, Json<SubmitScoresResponse>) {
    let count = body.scores.len();
    state.store.record_batch(body.scores);
    (
        StatusCode::CREATED,
        Json(SubmitScoresResponse { accepted: count }),
    )
}

async fn list_sessions(State(state): State<AppState>) -> Json<SessionsResponse> {
    let session_ids = state.store.session_ids();
    let sessions = session_ids
        .into_iter()
        .map(|id| {
            let score_count = state.store.get_session_scores(&id).len();
            let aggregate_quality = state.store.aggregate_quality(&id);
            SessionSummary {
                session_id: id,
                score_count,
                aggregate_quality,
            }
        })
        .collect();

    Json(SessionsResponse { sessions })
}

/// Build the Nous API router with a shared score store.
pub fn nous_router(store: ScoreStore, evaluator_count: u32) -> Router {
    let state = AppState {
        store,
        evaluator_count,
    };

    Router::new()
        .route("/health", get(health))
        .route("/eval/sessions", get(list_sessions))
        .route("/eval/scores", post(post_scores))
        .route("/eval/{session_id}", get(get_eval))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_app() -> Router {
        let store = ScoreStore::new();
        nous_router(store, 6)
    }

    #[tokio::test]
    async fn health_returns_ok_with_evaluator_count() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, "ok");
        assert_eq!(health.evaluator_count, 6);
    }

    #[tokio::test]
    async fn post_scores_then_get_eval() {
        let store = ScoreStore::new();
        let app = nous_router(store, 6);

        // Submit scores.
        let score_json = serde_json::json!({
            "scores": [
                {
                    "evaluator": "token_efficiency",
                    "value": 0.85,
                    "label": "good",
                    "layer": "execution",
                    "timing": "inline",
                    "explanation": null,
                    "session_id": "sess-42",
                    "run_id": null
                },
                {
                    "evaluator": "budget_adherence",
                    "value": 0.65,
                    "label": "warning",
                    "layer": "cost",
                    "timing": "inline",
                    "explanation": "budget near limit",
                    "session_id": "sess-42",
                    "run_id": null
                }
            ]
        });

        let post_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/eval/scores")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&score_json).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(post_response.status(), StatusCode::CREATED);

        let post_body = post_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let submit_resp: SubmitScoresResponse = serde_json::from_slice(&post_body).unwrap();
        assert_eq!(submit_resp.accepted, 2);

        // Query scores.
        let get_response = app
            .oneshot(
                Request::builder()
                    .uri("/eval/sess-42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);

        let get_body = get_response.into_body().collect().await.unwrap().to_bytes();
        let eval: EvalResponse = serde_json::from_slice(&get_body).unwrap();
        assert_eq!(eval.session_id, "sess-42");
        assert_eq!(eval.scores.len(), 2);
        assert!((eval.aggregate_quality - 0.75).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn get_eval_unknown_session_returns_empty() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/eval/unknown-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let eval: EvalResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(eval.session_id, "unknown-session");
        assert!(eval.scores.is_empty());
        assert!((eval.aggregate_quality).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn list_sessions_returns_submitted_sessions() {
        let store = ScoreStore::new();
        let app = nous_router(store.clone(), 6);

        // Submit to two sessions.
        let scores_json = serde_json::json!({
            "scores": [
                {
                    "evaluator": "a",
                    "value": 0.9,
                    "label": "good",
                    "layer": "action",
                    "timing": "inline",
                    "explanation": null,
                    "session_id": "s1",
                    "run_id": null
                },
                {
                    "evaluator": "b",
                    "value": 0.4,
                    "label": "critical",
                    "layer": "safety",
                    "timing": "async",
                    "explanation": null,
                    "session_id": "s2",
                    "run_id": null
                }
            ]
        });

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/eval/scores")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&scores_json).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // List sessions.
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/eval/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let sessions: SessionsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(sessions.sessions.len(), 2);

        let mut ids: Vec<&str> = sessions
            .sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["s1", "s2"]);
    }
}
