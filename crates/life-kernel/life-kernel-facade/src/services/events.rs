//! `life.Events` service adapter.
//!
//! Generic over `EventStorePort` so `lifed` can use `EventsProxy` (over lagod)
//! or a direct impl without changing the adapter.

use crate::convert::{from_json, kernel_err_to_status, to_json};
use aios_protocol::event::EventRecord;
use aios_protocol::ids::{BranchId, SessionId};
use aios_protocol::ports::EventStorePort;
use futures::StreamExt;
use life_kernel_proto::events as pb;
use pb::events_service_server::EventsService as TonicEvents;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::{Request, Response, Status};

/// Generic tonic service adapter for `life.Events`.
pub struct EventsService<P: EventStorePort + 'static> {
    port: Arc<P>,
}

impl<P: EventStorePort> EventsService<P> {
    /// Wrap a port impl in this adapter.
    pub fn new(port: Arc<P>) -> Self {
        Self { port }
    }
}

type SubStream =
    Pin<Box<dyn futures::Stream<Item = Result<pb::EventRecord, Status>> + Send + 'static>>;

/// Convert an `aios_protocol::event::EventRecord` to its proto wire type.
pub(crate) fn record_to_wire(r: &EventRecord) -> Result<pb::EventRecord, Status> {
    Ok(pb::EventRecord {
        event_id: r.event_id.to_string(),
        session_id: r.session_id.to_string(),
        branch_id: r.branch_id.to_string(),
        agent_id: r.agent_id.to_string(),
        sequence: r.sequence,
        recorded_at: Some(prost_types::Timestamp {
            seconds: r.timestamp.timestamp(),
            nanos: r.timestamp.timestamp_subsec_nanos() as i32,
        }),
        kind_json: to_json(&r.kind, "record.kind")?,
        causation_json: r
            .causation_id
            .as_ref()
            .map(|c| to_json(c, "record.causation_id"))
            .transpose()?,
    })
}

/// Convert a proto `EventRecord` back to `aios_protocol::event::EventRecord`.
pub(crate) fn wire_to_record(w: pb::EventRecord) -> Result<EventRecord, Status> {
    use aios_protocol::event::{EventActor, EventKind, EventSchema};
    use aios_protocol::ids::EventId;
    use chrono::{DateTime, Utc};

    let timestamp = w
        .recorded_at
        .ok_or_else(|| Status::invalid_argument("recorded_at required"))
        .and_then(|t| {
            DateTime::<Utc>::from_timestamp(t.seconds, t.nanos as u32)
                .ok_or_else(|| Status::invalid_argument("recorded_at invalid timestamp"))
        })?;

    let kind: EventKind = from_json(&w.kind_json, "kind_json")?;
    let causation_id = w
        .causation_json
        .as_deref()
        .map(|b| from_json(b, "causation_json"))
        .transpose()?;

    Ok(EventRecord {
        event_id: EventId::from(w.event_id),
        session_id: SessionId::from(w.session_id),
        agent_id: aios_protocol::ids::AgentId::from(w.agent_id),
        branch_id: BranchId::from(w.branch_id),
        sequence: w.sequence,
        timestamp,
        actor: EventActor::default(),
        schema: EventSchema::default(),
        causation_id,
        correlation_id: None,
        trace_id: None,
        span_id: None,
        digest: None,
        kind,
    })
}

#[tonic::async_trait]
impl<P: EventStorePort + Send + Sync + 'static> TonicEvents for EventsService<P> {
    async fn append(
        &self,
        req: Request<pb::AppendRequest>,
    ) -> Result<Response<pb::AppendResponse>, Status> {
        let r = req.into_inner();
        let rec = r
            .record
            .ok_or_else(|| Status::invalid_argument("record required"))?;
        let canonical = wire_to_record(rec)?;
        let stored = self
            .port
            .append(canonical)
            .await
            .map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::AppendResponse {
            record: Some(record_to_wire(&stored)?),
        }))
    }

    async fn read(
        &self,
        req: Request<pb::ReadRequest>,
    ) -> Result<Response<pb::ReadResponse>, Status> {
        let r = req.into_inner();
        let session = SessionId::from(r.session.unwrap_or_default().value);
        let branch = BranchId::from(r.branch.unwrap_or_default().value);
        let limit = r.limit.unwrap_or(256) as usize;
        let records = self
            .port
            .read(session, branch, r.after_sequence, limit)
            .await
            .map_err(kernel_err_to_status)?;
        let wire = records
            .iter()
            .map(record_to_wire)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Response::new(pb::ReadResponse { records: wire }))
    }

    async fn head(
        &self,
        req: Request<pb::HeadRequest>,
    ) -> Result<Response<pb::HeadResponse>, Status> {
        let r = req.into_inner();
        let session = SessionId::from(r.session.unwrap_or_default().value);
        let branch = BranchId::from(r.branch.unwrap_or_default().value);
        let head = self
            .port
            .head(session, branch)
            .await
            .map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::HeadResponse {
            head: Some(life_kernel_proto::common::SequenceNumber { value: head }),
        }))
    }

    type SubscribeStream = SubStream;

    async fn subscribe(
        &self,
        req: Request<pb::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let r = req.into_inner();
        let session = SessionId::from(r.session.unwrap_or_default().value);
        let branch = BranchId::from(r.branch.unwrap_or_default().value);
        let mut port_stream = self
            .port
            .subscribe(session, branch, r.after_sequence)
            .await
            .map_err(kernel_err_to_status)?;
        let (tx, rx) = mpsc::unbounded_channel::<Result<pb::EventRecord, Status>>();
        tokio::spawn(async move {
            while let Some(next) = port_stream.next().await {
                match next {
                    Ok(rec) => match record_to_wire(&rec) {
                        Ok(w) => {
                            if tx.send(Ok(w)).is_err() {
                                return;
                            }
                        }
                        Err(status) => {
                            let _ = tx.send(Err(status));
                            return;
                        }
                    },
                    Err(err) => {
                        let _ = tx.send(Err(kernel_err_to_status(err)));
                        return;
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(UnboundedReceiverStream::new(rx))))
    }
}
