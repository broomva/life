//! v0.2 reserved services — `life.Tools` / `life.Model` / `life.Relay`.
//!
//! Every method returns `Status::unimplemented`. The services are
//! registered on the wire so a v0 client talking to a v0.2 server
//! never sees a new service appear between tiers; only the methods'
//! availability changes. Full impls land when the corresponding port
//! traits are wire-projected in Spec B.1 Phase 2/4.

use life_kernel_proto::{model as mpb, relay as rpb, tools as tpb};
use std::pin::Pin;
use tonic::{Request, Response, Status};

/// v0.2 `life.Tools` stub.
pub struct ToolsService;

/// v0.2 `life.Model` stub.
pub struct ModelService;

/// v0.2 `life.Relay` stub.
pub struct RelayService;

#[tonic::async_trait]
impl tpb::tools_service_server::ToolsService for ToolsService {
    async fn execute(
        &self,
        _req: Request<tpb::ExecuteRequest>,
    ) -> Result<Response<tpb::ExecuteResponse>, Status> {
        Err(Status::unimplemented("life.Tools is reserved for v0.2"))
    }
}

#[tonic::async_trait]
impl mpb::model_service_server::ModelService for ModelService {
    async fn complete(
        &self,
        _req: Request<mpb::CompleteRequest>,
    ) -> Result<Response<mpb::CompleteResponse>, Status> {
        Err(Status::unimplemented("life.Model is reserved for v0.2"))
    }
}

type RelayStream =
    Pin<Box<dyn futures::Stream<Item = Result<rpb::RelayFrame, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl rpb::relay_service_server::RelayService for RelayService {
    async fn open(
        &self,
        _req: Request<rpb::OpenRequest>,
    ) -> Result<Response<rpb::OpenResponse>, Status> {
        Err(Status::unimplemented("life.Relay is reserved for v0.2"))
    }

    async fn send(
        &self,
        _req: Request<rpb::SendRequest>,
    ) -> Result<Response<rpb::SendResponse>, Status> {
        Err(Status::unimplemented("life.Relay is reserved for v0.2"))
    }

    type SubscribeStream = RelayStream;
    async fn subscribe(
        &self,
        _req: Request<rpb::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        Err(Status::unimplemented("life.Relay is reserved for v0.2"))
    }

    async fn close(
        &self,
        _req: Request<rpb::CloseRequest>,
    ) -> Result<Response<rpb::CloseResponse>, Status> {
        Err(Status::unimplemented("life.Relay is reserved for v0.2"))
    }
}
