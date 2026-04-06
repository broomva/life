//! HTTP handlers for event injection and schema discovery.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use opsis_core::event::OpsisEvent;
use opsis_core::feed::SchemaKey;
use opsis_core::schema::SchemaDefinition;

use crate::stream::AppState;

#[derive(Debug, Deserialize)]
pub struct InjectRequest {
    pub events: Vec<OpsisEvent>,
    #[serde(default)]
    pub source_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InjectResponse {
    pub accepted: usize,
    pub warnings: Vec<String>,
}

/// POST /events/inject — accepts external events into the bus.
pub async fn inject_events(
    State(state): State<AppState>,
    Json(request): Json<InjectRequest>,
) -> impl IntoResponse {
    let mut warnings = Vec::new();

    for event in &request.events {
        if !state.schema_registry.is_known(&event.schema_key) {
            warnings.push(format!("unknown schema_key: {}", event.schema_key));
        }
    }

    let accepted = request.events.len();
    for event in request.events {
        state.bus.publish_event(event);
    }

    (
        StatusCode::ACCEPTED,
        Json(InjectResponse { accepted, warnings }),
    )
}

/// GET /schemas — list all registered schemas.
pub async fn list_schemas(State(state): State<AppState>) -> Json<Vec<SchemaDefinition>> {
    Json(state.schema_registry.all().into_iter().cloned().collect())
}

/// GET /schemas/{key} — look up a single schema.
pub async fn get_schema(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    match state.schema_registry.lookup(&SchemaKey::new(&key)) {
        Some(def) => (StatusCode::OK, Json(serde_json::to_value(def).unwrap())).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
