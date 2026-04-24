//! `life.Session` service adapter.

use crate::convert::{from_json, kernel_err_to_status, to_json};
use aios_protocol::ids::{BranchId, SessionId};
use aios_protocol::ports::SessionPort;
use aios_protocol::session::{CreateSessionRequest, SessionFilter, TickInput};
use futures::StreamExt;
use life_kernel_proto::{events as evpb, session as pb};
use pb::session_service_server::SessionService as TonicSession;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::{Request, Response, Status};

/// Generic tonic service adapter for `life.Session`.
pub struct SessionService<P: SessionPort + 'static> {
    port: Arc<P>,
}

impl<P: SessionPort> SessionService<P> {
    /// Wrap a port impl in this adapter.
    pub fn new(port: Arc<P>) -> Self {
        Self { port }
    }
}

type SessionEventStream =
    Pin<Box<dyn futures::Stream<Item = Result<evpb::EventRecord, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl<P: SessionPort + Send + Sync + 'static> TonicSession for SessionService<P> {
    async fn create(
        &self,
        req: Request<pb::CreateRequest>,
    ) -> Result<Response<pb::SessionManifest>, Status> {
        let r = req.into_inner();
        let canonical: CreateSessionRequest = from_json(&r.request_json, "request_json")?;
        let manifest = self
            .port
            .create(canonical)
            .await
            .map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::SessionManifest {
            manifest_json: to_json(&manifest, "manifest")?,
        }))
    }

    async fn get(
        &self,
        req: Request<pb::GetRequest>,
    ) -> Result<Response<pb::SessionManifest>, Status> {
        let sid = SessionId::from(req.into_inner().session.unwrap_or_default().value);
        let manifest = self.port.get(sid).await.map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::SessionManifest {
            manifest_json: to_json(&manifest, "manifest")?,
        }))
    }

    async fn list(
        &self,
        req: Request<pb::ListRequest>,
    ) -> Result<Response<pb::ListResponse>, Status> {
        let filter: SessionFilter = req
            .into_inner()
            .filter
            .map(|f| from_json::<SessionFilter>(&f.filter_json, "filter_json"))
            .transpose()?
            .unwrap_or_default();
        let manifests = self.port.list(filter).await.map_err(kernel_err_to_status)?;
        let wire = manifests
            .iter()
            .map(|m| {
                Ok(pb::SessionManifest {
                    manifest_json: to_json(m, "manifest")?,
                })
            })
            .collect::<Result<Vec<_>, Status>>()?;
        Ok(Response::new(pb::ListResponse { manifests: wire }))
    }

    async fn tick(
        &self,
        req: Request<pb::TickRequest>,
    ) -> Result<Response<pb::TickResponse>, Status> {
        let r = req.into_inner();
        let sid = SessionId::from(r.session.unwrap_or_default().value);
        let input: TickInput = from_json(&r.input_json, "input_json")?;
        let output = self
            .port
            .tick(sid, input)
            .await
            .map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::TickResponse {
            output_json: to_json(&output, "output")?,
        }))
    }

    type StreamEventsStream = SessionEventStream;

    async fn stream_events(
        &self,
        req: Request<pb::StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let r = req.into_inner();
        let sid = SessionId::from(r.session.unwrap_or_default().value);
        let branch = BranchId::from(r.branch.unwrap_or_default().value);
        let mut port_stream = self
            .port
            .stream_events(sid, branch, r.after_sequence)
            .await
            .map_err(kernel_err_to_status)?;
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(next) = port_stream.next().await {
                match next {
                    Ok(rec) => match crate::services::events::record_to_wire(&rec) {
                        Ok(w) => {
                            if tx.send(Ok(w)).is_err() {
                                return;
                            }
                        }
                        Err(s) => {
                            let _ = tx.send(Err(s));
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

    async fn close(
        &self,
        req: Request<pb::CloseRequest>,
    ) -> Result<Response<pb::CloseResponse>, Status> {
        let r = req.into_inner();
        let sid = SessionId::from(r.session.unwrap_or_default().value);
        self.port
            .close(sid, r.reason)
            .await
            .map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::CloseResponse {}))
    }
}
