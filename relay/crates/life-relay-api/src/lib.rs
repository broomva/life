//! Life Relay API — local HTTP server for health checks and session management.
//!
//! The router is built with [`build_router`] and accepts an [`AppState`] that
//! holds the live session registry. The daemon populates this registry when
//! sessions are spawned or ended.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::{Json, Router, routing::get};
use life_relay_core::SessionInfo;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

/// Shared session registry populated by the daemon.
pub type SessionRegistry = Arc<RwLock<HashMap<Uuid, SessionInfo>>>;

/// State passed to every API handler.
#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionRegistry,
}

/// Build the relay daemon's local HTTP router.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/{id}", get(get_session))
        .with_state(state)
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

async fn list_sessions(State(state): State<AppState>) -> Json<Value> {
    let owned: Vec<SessionInfo> = state.sessions.read().await.values().cloned().collect();
    Json(json!({ "sessions": owned }))
}

async fn get_session(State(state): State<AppState>, Path(id): Path<Uuid>) -> Json<Value> {
    match state.sessions.read().await.get(&id).cloned() {
        Some(session) => Json(json!({ "session": session })),
        None => Json(json!({ "error": "session not found", "id": id })),
    }
}
