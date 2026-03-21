//! Nous HTTP routes.

use axum::{Router, extract::Path, response::Json, routing::get};
use serde::Serialize;

/// Response for the eval endpoint.
#[derive(Debug, Serialize)]
pub struct EvalResponse {
    pub session_id: String,
    pub scores: Vec<ScoreEntry>,
    pub aggregate_quality: f64,
}

/// A single score entry in the API response.
#[derive(Debug, Serialize)]
pub struct ScoreEntry {
    pub evaluator: String,
    pub value: f64,
    pub label: String,
    pub layer: String,
    pub explanation: Option<String>,
}

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub evaluator_count: u32,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        evaluator_count: 0,
    })
}

async fn get_eval(Path(session_id): Path<String>) -> Json<EvalResponse> {
    // Placeholder: will be wired to actual score storage.
    Json(EvalResponse {
        session_id,
        scores: vec![],
        aggregate_quality: 0.0,
    })
}

/// Build the Nous API router.
pub fn nous_router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/eval/{session_id}", get(get_eval))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let app = nous_router();
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
    }

    #[tokio::test]
    async fn eval_endpoint_returns_empty() {
        let app = nous_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/eval/test-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
