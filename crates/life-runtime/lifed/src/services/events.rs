//! life.v1.Events — public-plane events namespace.
//!
//! Sub-phase B drives events from the `lago-proxy::LagoCall` trait
//! directly. The mock lago substrate (in dev/tests) and the real
//! `LagoProxy` (in production) both implement `LagoCall`, so the
//! handler doesn't care which is wired in.
//!
//! ## Pool bracketing — Sub-phase E
//!
//! Sub-phase E pushes pool bracketing inside each proxy crate's
//! `Pooled<C>` adapter (Spec C₂ §7). Events handlers no longer need a
//! `pools` field — every `self.lago.<rpc>()` call brackets internally.

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tonic::{Request, Response, Status};

use lago_proxy::LagoCall;
use life_runtime_proto::life::v1 as pb;

use crate::auth::capability::CapabilityClaims;

pub struct EventsService {
    pub lago: Arc<dyn LagoCall>,
}

impl EventsService {
    pub fn new(lago: Arc<dyn LagoCall>) -> Self {
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
        let stream = self
            .lago
            .read(&sid, body.from_sequence, body.limit)
            .await
            .map_err(Status::from)?;
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
        let stream = self
            .lago
            .subscribe(&sid, body.from_sequence)
            .await
            .map_err(Status::from)?;
        Ok(Response::new(stream))
    }

    async fn get_blob(&self, req: Request<pb::BlobRef>) -> Result<Response<pb::Blob>, Status> {
        let _claims = Self::claims(&req)?;
        let body = req.get_ref();
        let (data, content_type) = self
            .lago
            .get_blob(&body.namespace, &body.sha256)
            .await
            .map_err(Status::from)?;
        Ok(Response::new(pb::Blob { data, content_type }))
    }
}
