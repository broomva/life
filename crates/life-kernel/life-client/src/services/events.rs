//! Typed handle over `life.Events`.
//!
//! # Example
//!
//! ```no_run
//! use life_client::{LifeClient, LifeTransport, LifeResult};
//! use aios_protocol::ids::{SessionId, BranchId};
//! use futures::StreamExt;
//!
//! # async fn demo() -> LifeResult<()> {
//! let client = LifeClient::connect(LifeTransport::Unix("/run/soma/sock".into())).await?;
//! let mut stream = client
//!     .events()
//!     .subscribe(SessionId::from("s-1"), BranchId::from("main"), 0)
//!     .await?;
//! while let Some(frame) = stream.next().await {
//!     let record = frame?;
//!     println!("seq={} kind={:?}", record.sequence, record.kind);
//! }
//! # Ok(()) }
//! ```

use crate::connect::LifeClient;
use crate::error::{LifeClientError, LifeResult};
use aios_protocol::event::{EventKind, EventRecord};
use aios_protocol::ids::{AgentId, BranchId, EventId, SessionId};
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use life_kernel_proto::{common, events as pb};
use pb::events_service_client::EventsServiceClient;
use std::pin::Pin;

/// Typed handle over the `life.Events` service.
pub struct Events<'a> {
    client: &'a LifeClient,
}

impl<'a> Events<'a> {
    /// Construct a new handle. Called from `LifeClient::events`.
    pub(crate) fn new(client: &'a LifeClient) -> Self {
        Self { client }
    }

    /// Get the current head sequence number for the (session, branch).
    pub async fn head(&self, session: SessionId, branch: BranchId) -> LifeResult<u64> {
        let mut c = EventsServiceClient::new(self.client.channel());
        let res = c
            .head(pb::HeadRequest {
                session: Some(common::SessionId {
                    value: session.to_string(),
                }),
                branch: Some(common::BranchId {
                    value: branch.to_string(),
                }),
            })
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?;
        Ok(res.into_inner().head.unwrap_or_default().value)
    }

    /// Read a page of events after a given sequence.
    pub async fn read(
        &self,
        session: SessionId,
        branch: BranchId,
        after_sequence: u64,
        limit: Option<u32>,
    ) -> LifeResult<Vec<EventRecord>> {
        let mut c = EventsServiceClient::new(self.client.channel());
        let res = c
            .read(pb::ReadRequest {
                session: Some(common::SessionId {
                    value: session.to_string(),
                }),
                branch: Some(common::BranchId {
                    value: branch.to_string(),
                }),
                after_sequence,
                limit,
            })
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?
            .into_inner();
        res.records.into_iter().map(decode_event_record).collect()
    }

    /// Subscribe to an event stream starting after the given sequence.
    pub async fn subscribe(
        &self,
        session: SessionId,
        branch: BranchId,
        after_sequence: u64,
    ) -> LifeResult<Pin<Box<dyn Stream<Item = LifeResult<EventRecord>> + Send + 'static>>> {
        let mut c = EventsServiceClient::new(self.client.channel());
        let stream = c
            .subscribe(pb::SubscribeRequest {
                session: Some(common::SessionId {
                    value: session.to_string(),
                }),
                branch: Some(common::BranchId {
                    value: branch.to_string(),
                }),
                after_sequence,
            })
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?
            .into_inner();
        Ok(Box::pin(stream.map(|item| {
            item.map_err(|e| LifeClientError::Rpc(e.to_string()))
                .and_then(decode_event_record)
        })))
    }
}

fn decode_event_record(w: pb::EventRecord) -> LifeResult<EventRecord> {
    let timestamp = w
        .recorded_at
        .and_then(|t| DateTime::<Utc>::from_timestamp(t.seconds, t.nanos as u32))
        .ok_or_else(|| LifeClientError::Rpc("invalid recorded_at".into()))?;
    let kind: EventKind = serde_json::from_slice(&w.kind_json)
        .map_err(|e| LifeClientError::Rpc(format!("kind: {e}")))?;
    let causation_id = w
        .causation_json
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|e| LifeClientError::Rpc(format!("causation: {e}")))?;
    Ok(EventRecord {
        event_id: EventId::from(w.event_id),
        session_id: SessionId::from(w.session_id),
        agent_id: AgentId::from(w.agent_id),
        branch_id: BranchId::from(w.branch_id),
        sequence: w.sequence,
        timestamp,
        actor: aios_protocol::event::EventActor::default(),
        schema: aios_protocol::event::EventSchema::default(),
        causation_id,
        correlation_id: None,
        trace_id: None,
        span_id: None,
        digest: None,
        kind,
    })
}
