//! Typed tonic client for the arcan substrate.
//!
//! Wraps the auto-generated arcan client (when arcan publishes its proto),
//! adds a Tier-3 token attachment helper, and exposes a small, lifed-tailored
//! call surface so handlers stay thin.
//!
//! NOTE: arcan does not yet publish its own proto — sub-phase B uses the
//! existing `life-kernel-facade` proxies (`SessionProxy`, `ApprovalsProxy`)
//! as the backing call surface, and the trait below abstracts that. When
//! arcan ships its `arcan-proto` crate, the real generated client replaces
//! the facade-proxy under the hood without touching `lifed`'s handlers.
//!
//! Sub-phase E: each `*Proxy` owns the `Arc<Pool>` per Spec C₂ §7. Every
//! per-RPC method internally calls `self.acquire().await?` so handler
//! code drops its `pools` field. For mocks, the [`Pooled<C>`] adapter
//! wraps any inner [`ArcanCall`] impl and applies the same pool
//! bracketing — lifed's bootstrap wraps both the real `ArcanProxy` and
//! `MockArcan` in `Pooled<...>` so the breaker exercises identical paths.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use aios_proto::aios::v1 as aios_v1;
use arcan_substrate_proto::arcan::v1::{
    self as arcan_pb, agent_substrate_client::AgentSubstrateClient,
};
use async_trait::async_trait;
use futures::Stream;
use life_runtime_pool::pool::{Pool, PoolGuard};
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::error::{ArcanProxyError, ArcanProxyResult};

#[derive(Clone)]
pub struct ArcanProxy {
    channel: Channel,
    token: Option<String>,
    /// Sub-phase E: the per-substrate connection pool. Every RPC method
    /// brackets through `self.acquire().await?` before issuing the
    /// underlying tonic call. `None` means the proxy was constructed
    /// for a unit test that doesn't care about pool semantics; in that
    /// case methods bypass bracketing.
    pool: Option<Arc<Pool>>,
}

impl ArcanProxy {
    /// Dial the arcan UDS socket and return a connected proxy.
    pub async fn connect(socket: PathBuf) -> ArcanProxyResult<Self> {
        let endpoint = Endpoint::try_from("http://[::]:0")
            .map_err(|e| ArcanProxyError::Transport(format!("endpoint: {e}")))?;
        let channel = endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let socket = socket.clone();
                async move {
                    let s = UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
                }
            }))
            .await
            .map_err(|e| ArcanProxyError::Transport(format!("connect: {e}")))?;
        Ok(Self {
            channel,
            token: None,
            pool: None,
        })
    }

    /// Sub-phase E: attach a per-substrate connection pool. Every RPC
    /// method bracket through this pool when present.
    pub fn with_pool(mut self, pool: Arc<Pool>) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Attach a Tier-3 substrate token to outgoing metadata.
    pub fn with_token(mut self, token: String) -> Self {
        self.token = Some(token);
        self
    }

    /// Access the underlying transport for tests / future tonic-client wiring.
    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    /// Tier-3 substrate token (if attached).
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Sub-phase D3: attach the Tier-3 substrate token to a tonic
    /// outgoing request as `authorization: Bearer <jws>`. Per Spec C₂
    /// §5.2 every substrate call carries the bearer so the substrate
    /// can verify against lifed's published JWKS.
    pub fn attach_token<T>(&self, req: &mut tonic::Request<T>) {
        if let Some(token) = &self.token
            && let Ok(value) = format!("Bearer {token}").parse()
        {
            req.metadata_mut().insert("authorization", value);
        }
    }

    async fn acquire_guard(&self) -> ArcanProxyResult<Option<PoolGuard>> {
        match &self.pool {
            Some(pool) => Ok(Some(pool.acquire().await.map_err(ArcanProxyError::from)?)),
            None => Ok(None),
        }
    }

    /// Create (or attach to) an agent on the substrate. BRO-1016
    /// wires this to the real `arcan.v1.AgentSubstrate.CreateAgent`
    /// RPC. The substrate is idempotent on `sid`, so re-issuing the
    /// call after a saga retry is safe.
    pub async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
        let guard = self.acquire_guard().await?;
        let mut client = AgentSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(arcan_pb::CreateAgentReq {
            sid: Some(aios_v1::SessionId {
                value: sid.to_owned(),
            }),
            label: String::new(),
        });
        self.attach_token(&mut req);
        let result = client
            .create_agent(req)
            .await
            .map_err(ArcanProxyError::from);
        match result {
            Ok(resp) => {
                let agent_id = resp.into_inner().agent_id;
                record_outcome(guard, true);
                Ok(agent_id)
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
    }

    /// Destroy an agent. BRO-1016 wires this to
    /// `arcan.v1.AgentSubstrate.DestroyAgent`. Idempotent — sessions
    /// that don't exist substrate-side return Ok(empty).
    pub async fn destroy_agent(&self, sid: &str) -> ArcanProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let mut client = AgentSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(arcan_pb::DestroyAgentReq {
            sid: Some(aios_v1::SessionId {
                value: sid.to_owned(),
            }),
        });
        self.attach_token(&mut req);
        let result = client
            .destroy_agent(req)
            .await
            .map_err(ArcanProxyError::from);
        match result {
            Ok(_) => {
                record_outcome(guard, true);
                Ok(())
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
    }

    /// Dispatch a message and stream the substrate's events back as
    /// `life.v1.AgentEvent`s. BRO-1016 wires this to
    /// `arcan.v1.AgentSubstrate.DispatchMessage` and translates the
    /// substrate-plane events into the public-plane shape. Phase 2
    /// (harness arc): TOKEN/FINISH/ERROR plus the tool lifecycle —
    /// TOOL_CALL_PENDING / TOOL_RESULT pass through with their
    /// structured payloads as `EventRecord`s (see
    /// [`crate::conversions::SubstrateEventTranslator`]).
    ///
    /// BRO-1206: `model` is accepted at the trait boundary so callers
    /// (lifed) can plumb a per-session override end-to-end without a
    /// trait signature change downstream. The arcan substrate wire
    /// (`arcan.v1.AgentSubstrate.DispatchMessage`) does NOT yet carry a
    /// `model` field — this proxy ignores the override when forwarding
    /// to a real arcand. Override-or-env-fallback is honored by the
    /// `VercelAiGatewayArcan` and `AnthropicArcan` HTTP-backed impls.
    ///
    /// Client tool definitions: each `tools` entry is JSON-serialized
    /// into `DispatchMessageReq.tool_definitions` (opaque bytes per the
    /// payload_json wire pattern) so the chat surface's tools reach the
    /// substrate. Entries that fail to serialize are skipped — a
    /// malformed definition must not poison the whole dispatch.
    ///
    /// BRO-1479: `branch` is forwarded verbatim onto
    /// `DispatchMessageReq.branch`. Empty ⇒ the substrate dispatches on
    /// `main` (backward-compatible). A non-empty value forks the
    /// session's event stream + filesystem manifest on that branch; the
    /// arcand substrate validates the name (`[a-zA-Z0-9_-]{1,64}`) at
    /// its trust boundary and rejects an invalid name with
    /// `INVALID_ARGUMENT`. This proxy passes the bytes through unaltered
    /// — validation is the substrate's responsibility (defence in depth:
    /// the public lifegw edge also pre-validates).
    pub async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
        _model: Option<&str>,
        branch: &str,
        tools: &[serde_json::Value],
    ) -> ArcanProxyResult<
        Pin<
            Box<
                dyn Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>>
                    + Send,
            >,
        >,
    > {
        // Streams hold the guard for the full lifetime; ownership
        // passes to `PoolGuardedStream` which records the outcome on
        // Drop.
        let guard = self.acquire_guard().await?;
        let mut client = AgentSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(arcan_pb::DispatchMessageReq {
            sid: Some(aios_v1::SessionId {
                value: sid.to_owned(),
            }),
            content: content.to_owned(),
            tool_definitions: serialize_tool_definitions(tools),
            branch: branch.to_owned(),
        });
        self.attach_token(&mut req);
        let upstream = match client.dispatch_message(req).await {
            Ok(resp) => resp.into_inner(),
            Err(s) => {
                let err = ArcanProxyError::from(s);
                record_outcome(guard, !err.is_retryable());
                return Err(err);
            }
        };

        // Map arcan.v1.AgentEvent → life.v1.AgentEvent at the wire
        // boundary. Phase 2 (harness arc): the translator builds a
        // structured `EventRecord` per event (session id, sequence,
        // kind tag, JSON payload) so token text and tool payloads
        // survive the hop — see `conversions::SubstrateEventTranslator`.
        use futures::StreamExt;
        let mut translator = crate::conversions::SubstrateEventTranslator::new(sid);
        let mapped = upstream.map(move |res| res.map(|evt| translator.translate(evt)));
        let inner = Box::pin(mapped);
        Ok(Box::pin(PoolGuardedStream::new(inner, guard)))
    }
}

/// Serialize client tool definitions for the substrate wire. Each
/// definition becomes one `tool_definitions` entry (JSON bytes).
/// Entries that fail to serialize are skipped so a single malformed
/// value can't poison the dispatch — `serde_json::Value` serialization
/// is effectively infallible, but the guard keeps the path total.
pub fn serialize_tool_definitions(tools: &[serde_json::Value]) -> Vec<Vec<u8>> {
    tools
        .iter()
        .filter_map(|t| serde_json::to_vec(t).ok())
        .collect()
}

/// Record a `PoolGuard` outcome based on whether the call succeeded
/// or whether the error is a non-retryable / permanent failure (the
/// breaker treats permanent faults as success — they aren't infra
/// problems — per Spec C₂ §7.2). Mirrors the policy applied by
/// `Pooled<C>::bracket`.
fn record_outcome(guard: Option<PoolGuard>, success_or_permanent: bool) {
    if let Some(g) = guard {
        if success_or_permanent {
            g.record_success();
        } else {
            g.record_failure();
        }
    }
}

/// Object-safe trait covering the lifed-relevant subset of arcan operations.
/// Used in `lifed::services::agent` so the integration tests can swap the
/// real proxy for a mock under test.
///
/// BRO-1206: `dispatch_message` takes an optional `model` override that
/// flows from `POST /v1/agent/create_session`'s `model` field through
/// lifed's routing cache. `None` means "use the backend's env default"
/// (`OPENAI_MODEL` / `ANTHROPIC_MODEL`). Mocks accept and ignore it; the
/// substrate-gRPC `ArcanProxy` accepts and ignores it (the arcan
/// substrate wire doesn't carry a model field yet); HTTP-backed impls
/// (`VercelAiGatewayArcan`, `AnthropicArcan`) honour the override.
///
/// `tools` carries the client-supplied tool definitions for this
/// dispatch (AI-SDK / OpenAI function shape: `{"name", "description",
/// "parameters"}`). Empty means "no client tools" — backends fall back
/// to their own tool registry (if any). The substrate-gRPC
/// `ArcanProxy` forwards them as `DispatchMessageReq.tool_definitions`
/// bytes; HTTP-backed impls inject them into the outbound provider
/// request body (`tools` array).
///
/// BRO-1479: `branch` selects the target branch for this dispatch's
/// ticks. Empty ⇒ `main` (backward-compatible — pre-BRO-1479 callers
/// never set it). The substrate-gRPC `ArcanProxy` forwards it onto
/// `DispatchMessageReq.branch`, where arcand validates + keys it into
/// the event journal and filesystem manifest. HTTP-backed impls
/// (`VercelAiGatewayArcan`, `AnthropicArcan`) have no branch concept on
/// the raw provider wire and ignore it; mocks accept and ignore it.
#[async_trait]
pub trait ArcanCall: Send + Sync {
    async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String>;
    async fn destroy_agent(&self, sid: &str) -> ArcanProxyResult<()>;
    async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
        model: Option<&str>,
        branch: &str,
        tools: &[serde_json::Value],
    ) -> ArcanProxyResult<
        Pin<
            Box<
                dyn Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>>
                    + Send,
            >,
        >,
    >;
}

#[async_trait]
impl ArcanCall for ArcanProxy {
    async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
        ArcanProxy::create_agent(self, sid).await
    }
    async fn destroy_agent(&self, sid: &str) -> ArcanProxyResult<()> {
        ArcanProxy::destroy_agent(self, sid).await
    }
    async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
        model: Option<&str>,
        branch: &str,
        tools: &[serde_json::Value],
    ) -> ArcanProxyResult<
        Pin<
            Box<
                dyn Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>>
                    + Send,
            >,
        >,
    > {
        ArcanProxy::dispatch_message(self, sid, content, model, branch, tools).await
    }
}

/// Sub-phase E: pool-bracketing adapter. Wraps any inner [`ArcanCall`]
/// (real proxy, mock, fake) and applies the [`Pool`] semaphore +
/// circuit-breaker bracketing on every method. lifed's bootstrap wraps
/// both `ArcanProxy` (production) and `MockArcan` (tests) in `Pooled`
/// so the breaker exercises identical code paths in both modes.
pub struct Pooled<C: ArcanCall> {
    inner: C,
    pool: Arc<Pool>,
}

impl<C: ArcanCall> Pooled<C> {
    pub fn new(inner: C, pool: Arc<Pool>) -> Self {
        Self { inner, pool }
    }

    pub fn into_inner(self) -> C {
        self.inner
    }

    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    async fn bracket<T, F>(&self, fut: F) -> ArcanProxyResult<T>
    where
        F: std::future::Future<Output = ArcanProxyResult<T>>,
    {
        let guard = self.pool.acquire().await.map_err(ArcanProxyError::from)?;
        match fut.await {
            Ok(v) => {
                guard.record_success();
                Ok(v)
            }
            Err(e) => {
                if e.is_retryable() {
                    guard.record_failure();
                } else {
                    // Permanent errors are not breaker fodder — they
                    // record success so the breaker doesn't trip on
                    // policy/auth misconfiguration.
                    guard.record_success();
                }
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<C: ArcanCall> ArcanCall for Pooled<C> {
    async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
        self.bracket(self.inner.create_agent(sid)).await
    }

    async fn destroy_agent(&self, sid: &str) -> ArcanProxyResult<()> {
        self.bracket(self.inner.destroy_agent(sid)).await
    }

    async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
        model: Option<&str>,
        branch: &str,
        tools: &[serde_json::Value],
    ) -> ArcanProxyResult<
        Pin<
            Box<
                dyn Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>>
                    + Send,
            >,
        >,
    > {
        // Streams hold the guard until the inner stream terminates.
        let guard = self.pool.acquire().await.map_err(ArcanProxyError::from)?;
        match self
            .inner
            .dispatch_message(sid, content, model, branch, tools)
            .await
        {
            Ok(stream) => Ok(Box::pin(PoolGuardedStream::new(stream, Some(guard)))),
            Err(e) => {
                if e.is_retryable() {
                    guard.record_failure();
                } else {
                    guard.record_success();
                }
                Err(e)
            }
        }
    }
}

/// Wraps an upstream stream with a [`PoolGuard`] that records the
/// terminal outcome on stream close. Used by both [`ArcanProxy`] (when
/// owning a pool directly) and [`Pooled`] (when wrapping a foreign
/// `ArcanCall`). Holds the guard until the inner stream reaches
/// `Poll::Ready(None)` or yields an `Err`. Out-of-band drops also
/// surface to the breaker via the guard's defensive `Drop` impl.
pub struct PoolGuardedStream<S>
where
    S: Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>>,
{
    inner: S,
    guard: Option<PoolGuard>,
    saw_error: bool,
}

impl<S> PoolGuardedStream<S>
where
    S: Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>>,
{
    pub fn new(inner: S, guard: Option<PoolGuard>) -> Self {
        Self {
            inner,
            guard,
            saw_error: false,
        }
    }
}

impl<S> Stream for PoolGuardedStream<S>
where
    S: Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>> + Unpin,
{
    type Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(None) => {
                if let Some(g) = this.guard.take() {
                    if this.saw_error {
                        g.record_failure();
                    } else {
                        g.record_success();
                    }
                }
                Poll::Ready(None)
            }
            Poll::Ready(Some(Ok(item))) => Poll::Ready(Some(Ok(item))),
            Poll::Ready(Some(Err(e))) => {
                this.saw_error = true;
                Poll::Ready(Some(Err(e)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_proxy_with_token(token: &str) -> ArcanProxy {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        ArcanProxy {
            channel,
            token: Some(token.to_string()),
            pool: None,
        }
    }

    #[tokio::test]
    async fn attach_token_sets_authorization_header() {
        let proxy = dummy_proxy_with_token("jws.payload.sig");
        let mut req = tonic::Request::new(());
        proxy.attach_token(&mut req);
        let auth = req.metadata().get("authorization").expect("authz set");
        assert_eq!(auth.to_str().unwrap(), "Bearer jws.payload.sig");
    }

    #[tokio::test]
    async fn attach_token_no_op_when_token_absent() {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        let proxy = ArcanProxy {
            channel,
            token: None,
            pool: None,
        };
        let mut req = tonic::Request::new(());
        proxy.attach_token(&mut req);
        assert!(req.metadata().get("authorization").is_none());
    }

    #[tokio::test]
    async fn pooled_brackets_create_agent_through_breaker() {
        use life_runtime_pool::breaker::BreakerState;
        use life_runtime_pool::pool::{Pool, SubstrateKind};

        struct OkArcan;
        #[async_trait]
        impl ArcanCall for OkArcan {
            async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
                Ok(format!("agent-{sid}"))
            }
            async fn destroy_agent(&self, _sid: &str) -> ArcanProxyResult<()> {
                Ok(())
            }
            async fn dispatch_message(
                &self,
                _sid: &str,
                _content: &str,
                _model: Option<&str>,
                _branch: &str,
                _tools: &[serde_json::Value],
            ) -> ArcanProxyResult<
                Pin<
                    Box<
                        dyn Stream<
                                Item = Result<
                                    life_runtime_proto::life::v1::AgentEvent,
                                    tonic::Status,
                                >,
                            > + Send,
                    >,
                >,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                drop(tx);
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
        }

        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        let pool = Arc::new(Pool::new(channel, 4, SubstrateKind::Arcan));
        let pooled = Pooled::new(OkArcan, Arc::clone(&pool));
        let agent_id = pooled.create_agent("sid-1").await.expect("create");
        assert_eq!(agent_id, "agent-sid-1");
        // A successful call leaves the breaker Closed.
        assert_eq!(pool.breaker_state(), BreakerState::Closed);
    }

    #[tokio::test]
    async fn pooled_records_failure_on_retryable_error() {
        use life_runtime_pool::breaker::{BreakerState, FAILURE_THRESHOLD};
        use life_runtime_pool::pool::{Pool, SubstrateKind};

        struct FlakyArcan;
        #[async_trait]
        impl ArcanCall for FlakyArcan {
            async fn create_agent(&self, _sid: &str) -> ArcanProxyResult<String> {
                Err(ArcanProxyError::Substrate(tonic::Status::unavailable(
                    "down",
                )))
            }
            async fn destroy_agent(&self, _sid: &str) -> ArcanProxyResult<()> {
                Ok(())
            }
            async fn dispatch_message(
                &self,
                _sid: &str,
                _content: &str,
                _model: Option<&str>,
                _branch: &str,
                _tools: &[serde_json::Value],
            ) -> ArcanProxyResult<
                Pin<
                    Box<
                        dyn Stream<
                                Item = Result<
                                    life_runtime_proto::life::v1::AgentEvent,
                                    tonic::Status,
                                >,
                            > + Send,
                    >,
                >,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                drop(tx);
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
        }

        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        let pool = Arc::new(Pool::new(channel, 4, SubstrateKind::Arcan));
        let pooled = Pooled::new(FlakyArcan, Arc::clone(&pool));
        for _ in 0..FAILURE_THRESHOLD {
            let _ = pooled.create_agent("sid-x").await;
        }
        assert_eq!(pool.breaker_state(), BreakerState::Open);
    }

    /// Tool definitions serialize to one JSON-bytes entry per value —
    /// the exact shape `DispatchMessageReq.tool_definitions` carries.
    #[test]
    fn serialize_tool_definitions_one_entry_per_tool() {
        let tools = vec![
            serde_json::json!({
                "name": "get_weather",
                "description": "Look up the weather",
                "parameters": {"type": "object", "properties": {"city": {"type": "string"}}},
            }),
            serde_json::json!({"name": "noop"}),
        ];
        let wire = serialize_tool_definitions(&tools);
        assert_eq!(wire.len(), 2);
        for (raw, original) in wire.iter().zip(&tools) {
            let decoded: serde_json::Value = serde_json::from_slice(raw).expect("valid JSON");
            assert_eq!(&decoded, original, "bytes round-trip to the original value");
        }
    }

    #[test]
    fn serialize_tool_definitions_empty_is_empty() {
        assert!(serialize_tool_definitions(&[]).is_empty());
    }

    /// Wire-shape guard: `DispatchMessageReq.tool_definitions` bytes
    /// survive a prost encode/decode round trip unchanged (additive
    /// field 3 on the substrate dispatch request).
    #[test]
    fn dispatch_message_req_proto_roundtrip_preserves_tool_definitions() {
        use prost::Message;
        let tools = vec![serde_json::json!({
            "name": "get_weather",
            "description": "Look up the weather",
            "parameters": {"type": "object"},
        })];
        let req = arcan_pb::DispatchMessageReq {
            sid: Some(aios_v1::SessionId {
                value: "sid-1".to_string(),
            }),
            content: "hello".to_string(),
            tool_definitions: serialize_tool_definitions(&tools),
            branch: String::new(),
        };
        let bytes = req.encode_to_vec();
        let decoded = arcan_pb::DispatchMessageReq::decode(bytes.as_slice()).expect("decode");
        assert_eq!(decoded.content, "hello");
        assert_eq!(decoded.tool_definitions.len(), 1);
        let tool: serde_json::Value =
            serde_json::from_slice(&decoded.tool_definitions[0]).expect("JSON");
        assert_eq!(tool["name"], "get_weather");
    }

    /// Wire-shape guard: `DispatchMessageReq.branch` survives a prost
    /// encode/decode round trip unchanged (additive field 4 on the
    /// substrate dispatch request, BRO-1479). An empty branch round-trips
    /// to empty — the substrate reads that as `main`, preserving
    /// pre-BRO-1479 wire compatibility.
    #[test]
    fn dispatch_message_req_proto_roundtrip_preserves_branch() {
        use prost::Message;
        // Non-empty branch survives the hop.
        let req = arcan_pb::DispatchMessageReq {
            sid: Some(aios_v1::SessionId {
                value: "sid-1".to_string(),
            }),
            content: "hello".to_string(),
            tool_definitions: Vec::new(),
            branch: "exp-1".to_string(),
        };
        let bytes = req.encode_to_vec();
        let decoded = arcan_pb::DispatchMessageReq::decode(bytes.as_slice()).expect("decode");
        assert_eq!(decoded.branch, "exp-1");

        // Empty branch round-trips to empty (⇒ main at the substrate).
        let req_default = arcan_pb::DispatchMessageReq {
            sid: None,
            content: String::new(),
            tool_definitions: Vec::new(),
            branch: String::new(),
        };
        let decoded_default =
            arcan_pb::DispatchMessageReq::decode(req_default.encode_to_vec().as_slice())
                .expect("decode default");
        assert!(decoded_default.branch.is_empty());
    }

    #[tokio::test]
    async fn pool_guarded_stream_records_success_on_close() {
        use life_runtime_pool::breaker::BreakerState;
        use life_runtime_pool::pool::{Pool, SubstrateKind};

        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        let pool = Arc::new(Pool::new(channel, 4, SubstrateKind::Arcan));
        let guard = pool.acquire().await.expect("acquire");
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        // Drop tx so the receiver immediately yields None.
        drop(tx);
        let mut stream =
            PoolGuardedStream::new(tokio_stream::wrappers::ReceiverStream::new(rx), Some(guard));
        use futures::StreamExt;
        let next = stream.next().await;
        assert!(next.is_none());
        // On terminal close the guard records success.
        assert_eq!(pool.breaker_state(), BreakerState::Closed);
    }
}
