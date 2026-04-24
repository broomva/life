//! lagod Events proxy — HTTP+SSE implementation of
//! `aios_protocol::ports::EventStorePort`.
//!
//! Lagod's event API (under `/v1/`):
//! - `POST /v1/sessions/{id}/events`           → append + return `{ seq }`
//! - `GET  /v1/sessions/{id}/events/read`      → batch read `Vec<EventEnvelope>`
//! - `GET  /v1/sessions/{id}/events/head`      → `{ seq }` current head
//! - `GET  /v1/sessions/{id}/events` (SSE)     → stream `EventEnvelope` frames

use aios_protocol::error::{KernelError, KernelResult};
use aios_protocol::event::{EventEnvelope, EventRecord};
use aios_protocol::ids::{BranchId, SeqNo, SessionId};
use aios_protocol::ports::{EventRecordStream, EventStorePort};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::warn;

use crate::error::{FacadeError, FacadeResult};
use crate::lagod::client::LagoClient;

// ─── Lagod wire types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct HeadSeqResponse {
    seq: SeqNo,
}

#[derive(Serialize)]
struct AppendEventRequest<'a> {
    event: &'a EventEnvelope,
}

#[derive(Deserialize)]
struct AppendEventResponse {
    seq: SeqNo,
}

// ─── EventRecord ↔ EventEnvelope helpers ──────────────────────────────────

/// Convert an `EventRecord` (canonical aios-protocol ergonomic type) into
/// the `EventEnvelope` wire format that lagod stores.
fn record_to_envelope(r: &EventRecord) -> EventEnvelope {
    r.to_envelope()
}

/// Convert an `EventEnvelope` (lagod storage format) into the canonical
/// `EventRecord` used by the port trait.
fn envelope_to_record(env: EventEnvelope) -> EventRecord {
    use chrono::{DateTime, Utc};
    // EventEnvelope.timestamp is microseconds since epoch.
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

// ─── EventsProxy ──────────────────────────────────────────────────────────

/// HTTP proxy for `aios_protocol::ports::EventStorePort` over lagod.
pub struct EventsProxy {
    client: LagoClient,
}

impl EventsProxy {
    /// Construct from a configured [`LagoClient`].
    pub fn new(client: LagoClient) -> Self {
        Self { client }
    }

    async fn do_head(&self, session: &SessionId, branch: &BranchId) -> FacadeResult<u64> {
        let path = format!("/v1/sessions/{}/events/head", session);
        let res = self
            .client
            .request(reqwest::Method::GET, &path)
            .query(&[("branch", branch.as_str())])
            .send()
            .await
            .map_err(|e| FacadeError::BackendUnavailable { daemon: "lagod", source: e.into() })?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = res.text().await.unwrap_or_default();
            return Err(FacadeError::BackendRejected { daemon: "lagod", status, message });
        }
        let body: HeadSeqResponse = res.json().await.map_err(|e| FacadeError::BackendProtocol {
            daemon: "lagod",
            reason: e.to_string(),
        })?;
        Ok(body.seq)
    }

    async fn do_read(
        &self,
        session: &SessionId,
        branch: &BranchId,
        after: u64,
        limit: Option<usize>,
    ) -> FacadeResult<Vec<EventRecord>> {
        let path = format!("/v1/sessions/{}/events/read", session);
        let mut req = self
            .client
            .request(reqwest::Method::GET, &path)
            .query(&[("branch", branch.as_str()), ("after_seq", &after.to_string())]);
        if let Some(n) = limit {
            req = req.query(&[("limit", n.to_string())]);
        }
        let res = req.send().await.map_err(|e| FacadeError::BackendUnavailable {
            daemon: "lagod",
            source: e.into(),
        })?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = res.text().await.unwrap_or_default();
            return Err(FacadeError::BackendRejected { daemon: "lagod", status, message });
        }
        let envelopes: Vec<EventEnvelope> =
            res.json().await.map_err(|e| FacadeError::BackendProtocol {
                daemon: "lagod",
                reason: e.to_string(),
            })?;
        Ok(envelopes.into_iter().map(envelope_to_record).collect())
    }

    async fn do_append(&self, event: EventRecord) -> FacadeResult<EventRecord> {
        let path = format!("/v1/sessions/{}/events", event.session_id);
        let envelope = record_to_envelope(&event);
        let body = AppendEventRequest { event: &envelope };
        let res = self
            .client
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .map_err(|e| FacadeError::BackendUnavailable { daemon: "lagod", source: e.into() })?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = res.text().await.unwrap_or_default();
            return Err(FacadeError::BackendRejected { daemon: "lagod", status, message });
        }
        let resp: AppendEventResponse =
            res.json().await.map_err(|e| FacadeError::BackendProtocol {
                daemon: "lagod",
                reason: e.to_string(),
            })?;
        // Return the record with the sequence assigned by lagod.
        Ok(EventRecord { sequence: resp.seq, ..event })
    }

    async fn do_subscribe(
        &self,
        session: SessionId,
        branch: BranchId,
        after: u64,
    ) -> FacadeResult<EventRecordStream> {
        let path = format!("/v1/sessions/{}/events", session);
        let res = self
            .client
            .request(reqwest::Method::GET, &path)
            .query(&[
                ("branch", branch.as_str()),
                ("after_seq", &after.to_string()),
            ])
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|e| FacadeError::BackendUnavailable { daemon: "lagod", source: e.into() })?;

        let bytes_stream = res
            .bytes_stream()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

        let events = bytes_stream.eventsource();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<KernelResult<EventRecord>>();
        tokio::spawn(async move {
            let mut events = events;
            while let Some(next) = events.next().await {
                match next {
                    Ok(ev) => {
                        match serde_json::from_str::<EventEnvelope>(&ev.data) {
                            Ok(envelope) => {
                                if tx.send(Ok(envelope_to_record(envelope))).is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "lagod SSE frame parse failed");
                                let _ = tx.send(Err(KernelError::Runtime(format!(
                                    "lagod SSE parse: {e}"
                                ))));
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(KernelError::Runtime(format!(
                            "lagod SSE stream broke: {e}"
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
impl EventStorePort for EventsProxy {
    async fn append(&self, event: EventRecord) -> KernelResult<EventRecord> {
        self.do_append(event).await.map_err(Into::into)
    }

    async fn read(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        from_sequence: u64,
        limit: usize,
    ) -> KernelResult<Vec<EventRecord>> {
        self.do_read(&session_id, &branch_id, from_sequence, Some(limit))
            .await
            .map_err(Into::into)
    }

    async fn head(&self, session_id: SessionId, branch_id: BranchId) -> KernelResult<u64> {
        self.do_head(&session_id, &branch_id).await.map_err(Into::into)
    }

    async fn subscribe(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        after_sequence: u64,
    ) -> KernelResult<EventRecordStream> {
        self.do_subscribe(session_id, branch_id, after_sequence)
            .await
            .map_err(Into::into)
    }
}
