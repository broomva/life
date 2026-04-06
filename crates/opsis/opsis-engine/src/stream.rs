//! SSE stream server — broadcasts [`WorldDelta`] to connected web clients.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::Json;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use futures_util::StreamExt;
use serde::Serialize;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::CorsLayer;

use opsis_core::subscription::{ClientId, Subscription};

use crate::bus::EventBus;
use crate::engine::SnapshotHandle;
use crate::inject;
use crate::registry::ClientRegistry;
use crate::schema_registry::SchemaRegistry;

/// Shared application state for axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub bus: Arc<EventBus>,
    pub registry: ClientRegistry,
    pub schema_registry: Arc<SchemaRegistry>,
    pub snapshot: SnapshotHandle,
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

/// Build the axum router with all endpoints.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/stream", get(sse_stream))
        .route("/snapshot", get(snapshot))
        .route("/events/inject", post(inject::inject_events))
        .route("/schemas", get(inject::list_schemas))
        .route("/schemas/{key}", get(inject::get_schema))
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

/// GET /snapshot — returns the current accumulated world state for client hydration.
async fn snapshot(
    State(state): State<AppState>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    match state.snapshot.read().await.as_ref() {
        Some(snap) => Json(snap.clone()).into_response(),
        None => (StatusCode::SERVICE_UNAVAILABLE, "no snapshot available yet").into_response(),
    }
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
