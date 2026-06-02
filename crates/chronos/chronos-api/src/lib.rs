//! chronos-api — HTTP wake ingest + agenda inspection for Chronos (M1).
//!
//! Two routes turn Chronos's wake stream from one source (heartbeat) into a multi-source surface:
//!
//! - `POST /v1/wake` — submit an intent. The intent is added to the durable agenda; if it is
//!   ready to fire now (`when_unix_ms` absent or in the past) a [`chronos_core::WakeEvent`] is
//!   pushed into the [`HttpTrigger`](chronos_core) via the wake channel.
//! - `GET /v1/agenda/{session_id}` — list a session's agenda items, in dispatch order.
//! - `GET /v1/health` — liveness probe.
//!
//! ## Dependency injection keeps this crate thin
//!
//! `chronos-api` depends only on `chronos-core` (plus axum/serde/tokio). It holds an
//! `Arc<dyn AgendaStore>` and an `mpsc::Sender<WakeEvent>` — both injected by `chronosd`, which is
//! the single place where the concrete `LagoAgendaStore` and the real `HttpTrigger` receiver are
//! wired together. The API never names `chronos-lago` or `chronos-triggers`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chronos_core::{
    AgendaItem, AgendaStore, ChronosError, NewAgendaItem, Priority, SessionId, WakeEvent,
    WakeSource,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Shared state for the chronos-api router. Cheaply cloneable (an `Arc` + a channel sender + an
/// id), so axum can clone it per request.
#[derive(Clone)]
pub struct ApiState {
    /// Durable agenda store. `InMemoryAgendaStore` in tests; `LagoAgendaStore` in `chronosd`.
    pub agenda: Arc<dyn AgendaStore>,
    /// Sender into the paired [`HttpTrigger`](chronos_core); pushing here fires a wake through the
    /// router. (`chronos-core` re-exports the event type the channel carries.)
    pub wake_tx: mpsc::Sender<WakeEvent>,
    /// Session an item is routed to when the request omits `session_id`. `chronosd` injects the
    /// `chronos.system` session so the API never hard-codes a routing constant.
    pub default_session: SessionId,
}

/// Build the chronos-api router with the supplied state.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/wake", post(post_wake))
        .route("/v1/agenda/{session_id}", get(get_agenda))
        .route("/v1/health", get(health))
        .with_state(state)
}

/// Bind to `addr` and serve until `shutdown` resolves (graceful drain).
pub async fn serve<F>(addr: SocketAddr, state: ApiState, shutdown: F) -> std::io::Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    info!(%local, "chronos-api listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

/// Request body for `POST /v1/wake`.
#[derive(Debug, Deserialize)]
pub struct WakeRequest {
    /// Target session; if omitted, the item routes to the daemon's default (system) session.
    #[serde(default)]
    pub session_id: Option<String>,
    /// What the agent should do when it wakes. Must be non-empty.
    pub intent: String,
    /// Dispatch priority (`urgent` | `normal` | `deferrable`). Defaults to `normal`.
    #[serde(default)]
    pub priority: Priority,
    /// Earliest fire time, ms since epoch. If set and in the future, the item is added to the
    /// agenda (pending, `not_before` = this) but NOT fired now.
    #[serde(default)]
    pub when_unix_ms: Option<i64>,
    /// Optional free-form source label, echoed into the wake payload (informational; the
    /// [`WakeSource`] is always `Http`).
    #[serde(default)]
    pub source: Option<String>,
}

/// Response body for `POST /v1/wake`.
#[derive(Debug, Serialize)]
pub struct WakeResponse {
    /// Id of the agenda item that was created.
    pub agenda_item_id: String,
    /// Whether a wake was fired now (`false` when the intent is scheduled for the future).
    pub fired: bool,
    /// Session the item was routed to.
    pub session_id: String,
}

/// Response body for `GET /v1/agenda/{session_id}`.
#[derive(Debug, Serialize)]
pub struct AgendaListResponse {
    /// The session whose agenda was listed.
    pub session_id: String,
    /// Number of items returned.
    pub count: usize,
    /// The items, in dispatch order.
    pub items: Vec<AgendaItem>,
}

async fn post_wake(
    State(state): State<ApiState>,
    Json(req): Json<WakeRequest>,
) -> Result<(StatusCode, Json<WakeResponse>), ApiError> {
    if req.intent.trim().is_empty() {
        return Err(ApiError::bad_request("intent must not be empty"));
    }

    // Resolve the target session: explicit non-empty value, else the injected default.
    let target: Option<SessionId> = req
        .session_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(SessionId::from_string);
    let agenda_session = target
        .clone()
        .unwrap_or_else(|| state.default_session.clone());

    let now = chronos_core::now_unix_ms();
    let ready_now = req.when_unix_ms.map(|w| w <= now).unwrap_or(true);

    // Add to the durable agenda (always Pending; not_before carries the schedule if any).
    let mut new_item =
        NewAgendaItem::new(agenda_session.clone(), req.intent.clone(), WakeSource::Http)
            .with_priority(req.priority);
    if let Some(when) = req.when_unix_ms {
        new_item = new_item.with_not_before(when);
    }
    let item_id = state.agenda.add(new_item).await.map_err(ApiError::from)?;

    // Fire the wake now only if the intent is ready.
    let fired = if ready_now {
        let mut wake = WakeEvent::new(WakeSource::Http).with_payload(serde_json::json!({
            "intent": req.intent,
            "agenda_item_id": item_id.as_str(),
            "priority": req.priority.as_str(),
            "source": req.source,
        }));
        if let Some(t) = target {
            wake = wake.with_target_session(t);
        }
        match state.wake_tx.send(wake).await {
            Ok(()) => true,
            Err(err) => {
                // Agenda item is durably recorded; only the immediate wake was lost. The
                // heartbeat / a future scheduler can still pick the item up.
                warn!(error = %err, "wake channel closed; item added but wake not fired");
                false
            }
        }
    } else {
        false
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(WakeResponse {
            agenda_item_id: item_id.0,
            fired,
            session_id: agenda_session.as_str().to_string(),
        }),
    ))
}

async fn get_agenda(
    State(state): State<ApiState>,
    Path(session_id): Path<String>,
) -> Result<Json<AgendaListResponse>, ApiError> {
    let session = SessionId::from_string(&session_id);
    let items = state.agenda.list(&session).await.map_err(ApiError::from)?;
    Ok(Json(AgendaListResponse {
        session_id,
        count: items.len(),
        items,
    }))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "chronos-api" }))
}

/// API error → HTTP response mapping.
#[derive(Debug)]
pub enum ApiError {
    /// Malformed request (400).
    BadRequest(String),
    /// Backing store / internal failure (500).
    Internal(String),
}

impl ApiError {
    fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }
}

impl From<ChronosError> for ApiError {
    fn from(err: ChronosError) -> Self {
        match err {
            ChronosError::NotFound(m) => ApiError::BadRequest(format!("not found: {m}")),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chronos_core::{AgendaItemState, InMemoryAgendaStore, SessionId, WakeSource};
    use tower::ServiceExt; // for `oneshot`

    use super::*;

    const DEFAULT_SESSION: &str = "chronos.system";

    fn test_state() -> (ApiState, mpsc::Receiver<WakeEvent>) {
        let (tx, rx) = mpsc::channel(16);
        let state = ApiState {
            agenda: Arc::new(InMemoryAgendaStore::new()),
            wake_tx: tx,
            default_session: SessionId::from_string(DEFAULT_SESSION),
        };
        (state, rx)
    }

    fn post(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn post_wake_adds_pending_item_and_fires() {
        let (state, mut rx) = test_state();
        let agenda = state.agenda.clone();
        let app = router(state);

        let resp = app
            .oneshot(post(
                "/v1/wake",
                serde_json::json!({ "intent": "rebuild index" }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let json = body_json(resp).await;
        assert_eq!(json["fired"], true);
        assert_eq!(json["session_id"], DEFAULT_SESSION);

        // A wake was pushed into the trigger channel.
        let wake = rx.try_recv().expect("a wake was fired");
        assert_eq!(wake.source, WakeSource::Http);
        assert_eq!(wake.payload["intent"], "rebuild index");

        // The agenda holds exactly one pending item in the default session.
        let items = agenda
            .list(&SessionId::from_string(DEFAULT_SESSION))
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, AgendaItemState::Pending);
        assert_eq!(items[0].source, WakeSource::Http);
    }

    #[tokio::test]
    async fn post_wake_routes_to_explicit_session() {
        let (state, mut rx) = test_state();
        let agenda = state.agenda.clone();
        let app = router(state);

        let resp = app
            .oneshot(post(
                "/v1/wake",
                serde_json::json!({ "intent": "do it", "session_id": "user-7", "priority": "urgent" }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let json = body_json(resp).await;
        assert_eq!(json["session_id"], "user-7");

        let wake = rx.try_recv().expect("fired");
        assert_eq!(
            wake.target_session.as_ref().map(|s| s.as_str()),
            Some("user-7")
        );
        assert_eq!(
            agenda
                .list(&SessionId::from_string("user-7"))
                .await
                .unwrap()
                .len(),
            1
        );
        // Nothing leaked into the default session.
        assert!(
            agenda
                .list(&SessionId::from_string(DEFAULT_SESSION))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn future_intent_is_added_but_not_fired() {
        let (state, mut rx) = test_state();
        let agenda = state.agenda.clone();
        let app = router(state);

        let future = chronos_core::now_unix_ms() + 60_000;
        let resp = app
            .oneshot(post(
                "/v1/wake",
                serde_json::json!({ "intent": "later", "when_unix_ms": future }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(body_json(resp).await["fired"], false);

        // No wake fired...
        assert!(rx.try_recv().is_err(), "future intent must not fire now");
        // ...but the item is durably scheduled (pending, not_before set).
        let items = agenda
            .list(&SessionId::from_string(DEFAULT_SESSION))
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, AgendaItemState::Pending);
        assert_eq!(items[0].not_before_unix_ms, Some(future));
        assert!(!items[0].is_ready(chronos_core::now_unix_ms()));
    }

    #[tokio::test]
    async fn empty_intent_is_rejected() {
        let (state, _rx) = test_state();
        let app = router(state);
        let resp = app
            .oneshot(post("/v1/wake", serde_json::json!({ "intent": "   " })))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_agenda_lists_items_in_dispatch_order() {
        let (state, _rx) = test_state();
        let app = router(state);

        for (intent, priority) in [("low", "deferrable"), ("mid", "normal"), ("top", "urgent")] {
            let r = app
                .clone()
                .oneshot(post(
                    "/v1/wake",
                    serde_json::json!({ "intent": intent, "session_id": "s", "priority": priority }),
                ))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::ACCEPTED);
        }

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/agenda/s")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["count"], 3);
        let order: Vec<&str> = json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["intent"].as_str().unwrap())
            .collect();
        assert_eq!(order, vec!["top", "mid", "low"]);
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (state, _rx) = test_state();
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await["status"], "ok");
    }
}
