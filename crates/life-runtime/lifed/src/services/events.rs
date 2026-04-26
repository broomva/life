//! life.v1.Events — public-plane events namespace.
//!
//! Sub-phase A returns canned empty streams to validate the wire shape.
//! Sub-phase B wires real lago tail via `lago-proxy`.

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tonic::{Request, Response, Status};

use life_runtime_proto::life::v1 as pb;

use crate::auth::capability::CapabilityClaims;

#[async_trait::async_trait]
pub trait LagoTail: Send + Sync + 'static {
    async fn read(
        &self,
        sid: &str,
        from: u64,
        limit: u32,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<pb::EventRecord, Status>> + Send>>, Status>;

    async fn subscribe(
        &self,
        sid: &str,
        from: u64,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<pb::EventRecord, Status>> + Send>>, Status>;

    async fn get_blob(&self, namespace: &str, sha256: &str) -> Result<(Vec<u8>, String), Status>;
}

pub struct EventsService {
    pub lago: Arc<dyn LagoTail>,
}

impl EventsService {
    pub fn new(lago: Arc<dyn LagoTail>) -> Self {
        Self { lago }
    }

    fn claims<T>(req: &Request<T>) -> Result<&CapabilityClaims, Status> {
        req.extensions()
            .get::<CapabilityClaims>()
            .ok_or_else(|| Status::unauthenticated("missing capability claims"))
    }
}

#[tonic::async_trait]
impl pb::events_server::Events for EventsService {
    type ReadStream = Pin<Box<dyn Stream<Item = Result<pb::EventRecord, Status>> + Send>>;
    type SubscribeStream = Self::ReadStream;

    async fn read(&self, req: Request<pb::ReadReq>) -> Result<Response<Self::ReadStream>, Status> {
        let _claims = Self::claims(&req)?;
        let body = req.get_ref();
        let sid = body
            .session_id
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("missing session_id"))?
            .value
            .clone();
        let stream = self.lago.read(&sid, body.from_sequence, body.limit).await?;
        Ok(Response::new(stream))
    }

    async fn subscribe(
        &self,
        req: Request<pb::SubscribeReq>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let _claims = Self::claims(&req)?;
        let body = req.get_ref();
        let sid = body
            .session_id
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("missing session_id"))?
            .value
            .clone();
        let stream = self.lago.subscribe(&sid, body.from_sequence).await?;
        Ok(Response::new(stream))
    }

    async fn get_blob(&self, req: Request<pb::BlobRef>) -> Result<Response<pb::Blob>, Status> {
        let _claims = Self::claims(&req)?;
        let body = req.get_ref();
        let (data, content_type) = self.lago.get_blob(&body.namespace, &body.sha256).await?;
        Ok(Response::new(pb::Blob { data, content_type }))
    }
}
