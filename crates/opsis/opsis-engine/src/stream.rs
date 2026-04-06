//! SSE stream server — broadcasts [`WorldDelta`] to connected web clients.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::Json;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use futures_util::StreamExt;
use serde::Serialize;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::CorsLayer;

use opsis_core::subscription::{ClientId, Subscription};

use crate::bus::EventBus;
use crate::registry::ClientRegistry;

/// Shared application state for axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub bus: Arc<EventBus>,
    pub registry: ClientRegistry,
    pub started_at: std::time::Instant,
}

/// Health check response.
#[derive(Serialize)]
struct HealthResponse {
    status: String,
    service: String,
    version: String,
    uptime_seconds: u64,
    connected_clients: usize,
}

/// Build the axum router with `/health` and `/stream` endpoints.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/stream", get(sse_stream))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        service: "opsis".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        uptime_seconds: state.started_at.elapsed().as_secs(),
        connected_clients: state.registry.client_count().await,
    })
}

async fn sse_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let client_id = ClientId::default();
    state
        .registry
        .register(client_id.clone(), Subscription::all())
        .await;

    let delta_rx = state.bus.subscribe_deltas();
    let _registry = state.registry.clone();
    let _cid = client_id;

    let stream = BroadcastStream::new(delta_rx).map(|result| match result {
        Ok(delta) => {
            let json = serde_json::to_string(&delta).unwrap_or_default();
            Ok(Event::default().event("world_delta").data(json))
        }
        Err(_) => Ok(Event::default()
            .event("lagged")
            .data("events were dropped — consider reconnecting")),
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
