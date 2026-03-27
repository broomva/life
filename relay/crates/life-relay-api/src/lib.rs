//! Life Relay API — local HTTP server for health checks and session management.

use axum::{Json, Router, routing::get};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Build the relay daemon's local HTTP router.
pub fn build_router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/sessions", get(list_sessions))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "life-relay",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn list_sessions() -> Json<Value> {
    // Placeholder — will be wired to the daemon's session registry
    Json(json!({ "sessions": [] }))
}
