//! Transparent forwarding proxies for `life.v1.{Agent, Events, Wallet, Identity}`.
//!
//! Sub-phase A scope (Spec C₃ §12.A): every public-plane unary RPC the
//! gateway accepts is forwarded verbatim to lifed via the upstream UDS
//! channel. Server-streaming RPCs (`Agent.SendMessage`, `Agent.StreamSession`,
//! `Events.Read`, `Events.Subscribe`, `Wallet.Statement`) are passed through
//! using tonic's native `Streaming<T>`. The handlers do **not** perform any
//! business logic — they are pure transport translators.
//!
//! Per Spec C₃ §3.2: if a method is not implemented by lifed the gateway
//! returns the substrate's error verbatim — `Status::unimplemented` is not
//! rewritten.

use std::pin::Pin;

use futures::stream::Stream;
use tonic::transport::Channel;
use tonic::{Request, Response, Status};

use life_runtime_proto::life::v1 as pb;

/// Forwarder for `life.v1.Agent`. Holds an upstream tonic channel to
/// lifed.
#[derive(Clone)]
#[non_exhaustive]
pub struct AgentForwarder {
    channel: Channel,
}

impl AgentForwarder {
    pub fn new(channel: Channel) -> Self {
        Self { channel }
    }

    fn client(&self) -> pb::agent_client::AgentClient<Channel> {
        pb::agent_client::AgentClient::new(self.channel.clone())
    }
}

type AgentEventStream =
    Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl pb::agent_server::Agent for AgentForwarder {
    async fn create_session(
        &self,
        req: Request<pb::CreateSessionReq>,
    ) -> Result<Response<pb::Session>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .create_session(forwarded.into_request_with(body))
            .await
    }

    async fn describe_session(
        &self,
        req: Request<pb::SessionRef>,
    ) -> Result<Response<pb::Session>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .describe_session(forwarded.into_request_with(body))
            .await
    }

    async fn close_session(
        &self,
        req: Request<pb::SessionRef>,
    ) -> Result<Response<pb::Empty>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .close_session(forwarded.into_request_with(body))
            .await
    }

    type SendMessageStream = AgentEventStream;
    async fn send_message(
        &self,
        req: Request<pb::SendMessageReq>,
    ) -> Result<Response<Self::SendMessageStream>, Status> {
        let (forwarded, body) = forward_request(req);
        let resp = self
            .client()
            .send_message(forwarded.into_request_with(body))
            .await?;
        let (meta, stream, ext) = resp.into_parts();
        let pinned: AgentEventStream = Box::pin(stream);
        Ok(Response::from_parts(meta, pinned, ext))
    }

    type StreamSessionStream = AgentEventStream;
    async fn stream_session(
        &self,
        req: Request<pb::SessionRef>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        let (forwarded, body) = forward_request(req);
        let resp = self
            .client()
            .stream_session(forwarded.into_request_with(body))
            .await?;
        let (meta, stream, ext) = resp.into_parts();
        let pinned: AgentEventStream = Box::pin(stream);
        Ok(Response::from_parts(meta, pinned, ext))
    }

    async fn approve_dispatch(
        &self,
        req: Request<pb::ApprovalReq>,
    ) -> Result<Response<pb::Empty>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .approve_dispatch(forwarded.into_request_with(body))
            .await
    }

    async fn cancel_dispatch(
        &self,
        req: Request<pb::DispatchRef>,
    ) -> Result<Response<pb::Empty>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .cancel_dispatch(forwarded.into_request_with(body))
            .await
    }

    async fn list_skills(
        &self,
        req: Request<pb::ListSkillsReq>,
    ) -> Result<Response<pb::SkillCatalog>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .list_skills(forwarded.into_request_with(body))
            .await
    }

    async fn list_models(
        &self,
        req: Request<pb::ListModelsReq>,
    ) -> Result<Response<pb::ModelCatalog>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .list_models(forwarded.into_request_with(body))
            .await
    }

    async fn list_tools(
        &self,
        req: Request<pb::ListToolsReq>,
    ) -> Result<Response<pb::ToolCatalog>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .list_tools(forwarded.into_request_with(body))
            .await
    }

    async fn spawn_child(
        &self,
        req: Request<pb::SpawnChildReq>,
    ) -> Result<Response<pb::SpawnChildResp>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .spawn_child(forwarded.into_request_with(body))
            .await
    }
}

/// Forwarder for `life.v1.Events`.
#[derive(Clone)]
#[non_exhaustive]
pub struct EventsForwarder {
    channel: Channel,
}

impl EventsForwarder {
    pub fn new(channel: Channel) -> Self {
        Self { channel }
    }

    fn client(&self) -> pb::events_client::EventsClient<Channel> {
        pb::events_client::EventsClient::new(self.channel.clone())
    }
}

type EventRecordStream =
    Pin<Box<dyn Stream<Item = Result<pb::EventRecord, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl pb::events_server::Events for EventsForwarder {
    type ReadStream = EventRecordStream;
    async fn read(&self, req: Request<pb::ReadReq>) -> Result<Response<Self::ReadStream>, Status> {
        let (forwarded, body) = forward_request(req);
        let resp = self
            .client()
            .read(forwarded.into_request_with(body))
            .await?;
        let (meta, stream, ext) = resp.into_parts();
        let pinned: EventRecordStream = Box::pin(stream);
        Ok(Response::from_parts(meta, pinned, ext))
    }

    type SubscribeStream = EventRecordStream;
    async fn subscribe(
        &self,
        req: Request<pb::SubscribeReq>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let (forwarded, body) = forward_request(req);
        let resp = self
            .client()
            .subscribe(forwarded.into_request_with(body))
            .await?;
        let (meta, stream, ext) = resp.into_parts();
        let pinned: EventRecordStream = Box::pin(stream);
        Ok(Response::from_parts(meta, pinned, ext))
    }

    async fn get_blob(&self, req: Request<pb::BlobRef>) -> Result<Response<pb::Blob>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .get_blob(forwarded.into_request_with(body))
            .await
    }
}

/// Forwarder for `life.v1.Wallet`.
#[derive(Clone)]
#[non_exhaustive]
pub struct WalletForwarder {
    channel: Channel,
}

impl WalletForwarder {
    pub fn new(channel: Channel) -> Self {
        Self { channel }
    }

    fn client(&self) -> pb::wallet_client::WalletClient<Channel> {
        pb::wallet_client::WalletClient::new(self.channel.clone())
    }
}

type LedgerEntryStream =
    Pin<Box<dyn Stream<Item = Result<pb::LedgerEntry, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl pb::wallet_server::Wallet for WalletForwarder {
    async fn get_balance(
        &self,
        req: Request<pb::WalletRef>,
    ) -> Result<Response<pb::Balance>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .get_balance(forwarded.into_request_with(body))
            .await
    }

    type StatementStream = LedgerEntryStream;
    async fn statement(
        &self,
        req: Request<pb::StatementReq>,
    ) -> Result<Response<Self::StatementStream>, Status> {
        let (forwarded, body) = forward_request(req);
        let resp = self
            .client()
            .statement(forwarded.into_request_with(body))
            .await?;
        let (meta, stream, ext) = resp.into_parts();
        let pinned: LedgerEntryStream = Box::pin(stream);
        Ok(Response::from_parts(meta, pinned, ext))
    }

    async fn debit(
        &self,
        req: Request<pb::DebitReq>,
    ) -> Result<Response<pb::DebitReceipt>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client().debit(forwarded.into_request_with(body)).await
    }

    async fn transfer(
        &self,
        req: Request<pb::TransferReq>,
    ) -> Result<Response<pb::TransferReceipt>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .transfer(forwarded.into_request_with(body))
            .await
    }

    async fn x402_pay(
        &self,
        req: Request<pb::X402PayReq>,
    ) -> Result<Response<pb::X402PayResp>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .x402_pay(forwarded.into_request_with(body))
            .await
    }
}

/// Forwarder for `life.v1.Identity`.
#[derive(Clone)]
#[non_exhaustive]
pub struct IdentityForwarder {
    channel: Channel,
}

impl IdentityForwarder {
    pub fn new(channel: Channel) -> Self {
        Self { channel }
    }

    fn client(&self) -> pb::identity_client::IdentityClient<Channel> {
        pb::identity_client::IdentityClient::new(self.channel.clone())
    }
}

#[tonic::async_trait]
impl pb::identity_server::Identity for IdentityForwarder {
    async fn me(&self, req: Request<pb::IdentityEmpty>) -> Result<Response<pb::Account>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client().me(forwarded.into_request_with(body)).await
    }

    async fn update_profile(
        &self,
        req: Request<pb::UpdateProfileReq>,
    ) -> Result<Response<pb::Account>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .update_profile(forwarded.into_request_with(body))
            .await
    }

    async fn list_sessions(
        &self,
        req: Request<pb::ListSessionsReq>,
    ) -> Result<Response<pb::SessionList>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .list_sessions(forwarded.into_request_with(body))
            .await
    }

    async fn revoke_session(
        &self,
        req: Request<pb::IdentitySessionRef>,
    ) -> Result<Response<pb::IdentityEmpty>, Status> {
        let (forwarded, body) = forward_request(req);
        self.client()
            .revoke_session(forwarded.into_request_with(body))
            .await
    }
}

// ── Forwarding helpers ──────────────────────────────────────────────────

/// A request whose metadata + extensions have been peeled off the inbound
/// request, ready to be re-attached to the outbound upstream call.
struct ForwardedHeaders {
    meta: tonic::metadata::MetadataMap,
    ext: tonic::Extensions,
}

impl ForwardedHeaders {
    /// Re-attach the captured metadata + extensions to a freshly-constructed
    /// `Request` carrying the forwarded body.
    fn into_request_with<T>(self, body: T) -> Request<T> {
        let mut req = Request::new(body);
        *req.metadata_mut() = self.meta;
        *req.extensions_mut() = self.ext;
        req
    }
}

/// Split an inbound `Request<T>` into its metadata + extensions and the body
/// payload. The metadata MUST be carried through verbatim so that the
/// auth-middleware-rewritten `authorization` header reaches lifed and tonic's
/// trace-context propagation continues working.
fn forward_request<T>(req: Request<T>) -> (ForwardedHeaders, T) {
    let (meta, ext, body) = req.into_parts();
    (ForwardedHeaders { meta, ext }, body)
}

/// Dial a tonic Channel over a Unix Domain Socket. The same pattern lifed
/// uses for its admin client (Spec C₂ §12 admin plane).
pub async fn connect_uds(path: &std::path::Path) -> crate::error::LifegwResult<Channel> {
    use tokio::net::UnixStream;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let socket = path.to_path_buf();
    let endpoint = Endpoint::try_from("http://[::]:0")
        .map_err(|e| crate::error::LifegwError::Upstream(format!("endpoint scheme: {e}")))?;
    endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let socket = socket.clone();
            async move {
                let s = UnixStream::connect(socket).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
            }
        }))
        .await
        .map_err(|e| crate::error::LifegwError::Upstream(format!("dial uds: {e}")))
}
