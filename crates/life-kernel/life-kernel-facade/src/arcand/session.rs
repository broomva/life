//! arcand SessionPort proxy.
//!
//! Route map (arcand has no `/v1` prefix):
//! - `POST /sessions`                          → create (returns `SessionManifest`)
//! - `GET  /sessions`                          → list (returns `Vec<SessionSummary>`)
//! - `POST /sessions/{id}/runs`                → tick (arcand RunRequest → RunResponse)
//! - `GET  /sessions/{id}/events/stream` (SSE) → stream_events
//! - close: no explicit DELETE; arcand cleans up at restart — returns Ok(())

use crate::arcand::client::ArcanClient;
use crate::error::{FacadeError, FacadeResult};
use aios_protocol::error::{KernelError, KernelResult};
use aios_protocol::ids::{BranchId, SessionId};
use aios_protocol::ports::{EventRecordStream, SessionPort};
use aios_protocol::session::{
    CreateSessionRequest, ModelRouting, SessionFilter, SessionManifest, TickInput, TickOutput,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use tracing::warn;

use aios_protocol::event::{EventEnvelope, EventRecord};
use tokio_stream::wrappers::UnboundedReceiverStream;

// ─── arcand wire types ─────────────────────────────────────────────────────

/// arcand's `POST /sessions` request body.
#[derive(Debug, Serialize)]
struct ArcanCreateSessionRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_routing: Option<serde_json::Value>,
}

/// arcand's `POST /sessions/{id}/runs` request body.
#[derive(Debug, Serialize)]
struct ArcanRunRequest<'a> {
    objective: &'a str,
}

/// arcand's `POST /sessions/{id}/runs` response.
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fields reserved for richer TickOutput mapping in a later phase.
struct ArcanRunResponse {
    session_id: String,
    #[serde(default)]
    events_emitted: u64,
    #[serde(default)]
    last_sequence: u64,
}

/// arcand's `GET /sessions` summary item.
#[derive(Debug, Deserialize)]
struct ArcanSessionSummary {
    session_id: String,
    owner: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

// ─── helpers ──────────────────────────────────────────────────────────────

fn summary_to_manifest(s: ArcanSessionSummary) -> SessionManifest {
    SessionManifest {
        session_id: SessionId::from(s.session_id),
        owner: s.owner,
        created_at: s.created_at,
        // workspace_root and model_routing/policy are arcand-internal and
        // not exposed in the list endpoint. Use safe defaults.
        workspace_root: String::new(),
        model_routing: ModelRouting::default(),
        policy: serde_json::Value::Null,
    }
}

fn envelope_to_record(env: EventEnvelope) -> EventRecord {
    use chrono::{DateTime, Utc};
    let timestamp = DateTime::<Utc>::from_timestamp(
        (env.timestamp / 1_000_000) as i64,
        ((env.timestamp % 1_000_000) * 1_000) as u32,
    )
    .unwrap_or_else(Utc::now);
    EventRecord {
        event_id: env.event_id,
        session_id: env.session_id,
        agent_id: env.agent_id,
        branch_id: env.branch_id,
        sequence: env.seq,
        timestamp,
        actor: env.actor,
        schema: env.schema,
        causation_id: env.parent_id,
        correlation_id: None,
        trace_id: env.trace_id,
        span_id: env.span_id,
        digest: env.digest,
        kind: env.kind,
    }
}

// ─── SessionProxy ─────────────────────────────────────────────────────────

/// HTTP proxy for `aios_protocol::ports::SessionPort` over arcand.
pub struct SessionProxy {
    client: ArcanClient,
}

impl SessionProxy {
    /// Construct from a configured [`ArcanClient`].
    pub fn new(client: ArcanClient) -> Self {
        Self { client }
    }

    async fn http_json<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl serde::Serialize>,
    ) -> FacadeResult<T> {
        let mut req = self.client.request(method, path);
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req
            .send()
            .await
            .map_err(|e| FacadeError::BackendUnavailable {
                daemon: "arcand",
                source: e.into(),
            })?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = res.text().await.unwrap_or_default();
            return Err(FacadeError::BackendRejected {
                daemon: "arcand",
                status,
                message,
            });
        }
        res.json::<T>()
            .await
            .map_err(|e| FacadeError::BackendProtocol {
                daemon: "arcand",
                reason: e.to_string(),
            })
    }

    async fn do_stream(
        &self,
        id: SessionId,
        _branch: BranchId,
        after: u64,
    ) -> FacadeResult<EventRecordStream> {
        let path = format!("/sessions/{}/events/stream", id);
        let res = self
            .client
            .request(reqwest::Method::GET, &path)
            .query(&[("after_seq", after.to_string())])
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|e| FacadeError::BackendUnavailable {
                daemon: "arcand",
                source: e.into(),
            })?;

        let bytes = res.bytes_stream().map_err(std::io::Error::other);
        let events = bytes.eventsource();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<KernelResult<EventRecord>>();
        tokio::spawn(async move {
            let mut events = events;
            while let Some(next) = events.next().await {
                match next {
                    Ok(ev) => match serde_json::from_str::<EventEnvelope>(&ev.data) {
                        Ok(envelope) => {
                            if tx.send(Ok(envelope_to_record(envelope))).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            warn!(error=%e, "arcand SSE parse fail");
                            let _ = tx
                                .send(Err(KernelError::Runtime(format!("arcand SSE parse: {e}"))));
                            return;
                        }
                    },
                    Err(e) => {
                        let _ = tx.send(Err(KernelError::Runtime(format!(
                            "arcand SSE stream broke: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        Ok(Box::pin(UnboundedReceiverStream::new(rx)))
    }
}

#[async_trait]
impl SessionPort for SessionProxy {
    async fn create(&self, req: CreateSessionRequest) -> KernelResult<SessionManifest> {
        let body = ArcanCreateSessionRequest {
            owner: Some(req.owner.as_str()),
            model_routing: None,
        };
        self.http_json::<SessionManifest>(reqwest::Method::POST, "/sessions", Some(&body))
            .await
            .map_err(Into::into)
    }

    async fn get(&self, id: SessionId) -> KernelResult<SessionManifest> {
        // arcand has no per-session manifest endpoint; fetch list and filter.
        let all: Vec<ArcanSessionSummary> = self
            .http_json(reqwest::Method::GET, "/sessions", None::<&()>)
            .await
            .map_err(|e: FacadeError| KernelError::from(e))?;
        all.into_iter()
            .find(|s| s.session_id == id.as_str())
            .map(summary_to_manifest)
            .ok_or_else(|| KernelError::Runtime(format!("session not found: {id}")))
    }

    async fn list(&self, _filter: SessionFilter) -> KernelResult<Vec<SessionManifest>> {
        let all: Vec<ArcanSessionSummary> = self
            .http_json(reqwest::Method::GET, "/sessions", None::<&()>)
            .await
            .map_err(|e: FacadeError| KernelError::from(e))?;
        // Phase 1: client-side filter no-op (arcand has no server-side filter).
        Ok(all.into_iter().map(summary_to_manifest).collect())
    }

    async fn tick(&self, id: SessionId, input: TickInput) -> KernelResult<TickOutput> {
        let path = format!("/sessions/{}/runs", id);
        let body = ArcanRunRequest {
            objective: input.objective.as_str(),
        };
        let resp: ArcanRunResponse = self
            .http_json(reqwest::Method::POST, &path, Some(&body))
            .await
            .map_err(|e: FacadeError| KernelError::from(e))?;
        // TickOutput is #[non_exhaustive] — use JSON round-trip via serde.
        let json = serde_json::json!({
            "session_id": resp.session_id,
            "iteration": resp.events_emitted as u32,
            "stop_reason": "completed",
            "final_answer": null,
            "usage": null
        });
        serde_json::from_value(json)
            .map_err(|e| KernelError::Serialization(format!("TickOutput: {e}")))
    }

    async fn stream_events(
        &self,
        id: SessionId,
        branch: BranchId,
        after_sequence: u64,
    ) -> KernelResult<EventRecordStream> {
        self.do_stream(id, branch, after_sequence)
            .await
            .map_err(Into::into)
    }

    async fn close(&self, _id: SessionId, _reason: String) -> KernelResult<()> {
        // arcand has no explicit session-close HTTP endpoint in v0.
        // Sessions are memory-resident; they close at daemon restart.
        // Return Ok(()) rather than an error — callers tolerate no-op close.
        Ok(())
    }
}
