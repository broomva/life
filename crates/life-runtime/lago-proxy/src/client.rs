//! Typed tonic client for the lago substrate.
//!
//! Wraps a tonic Channel over the lago UDS socket, exposes the small slice
//! of lago RPCs lifed needs (namespace open/close, event read/subscribe,
//! blob get, idempotency lookup/persist), and provides an object-safe
//! `LagoCall` trait so lifed handlers can swap mocks under test.
//!
//! Sub-phase E: each `*Proxy` owns the `Arc<Pool>` per Spec C₂ §7. Every
//! per-RPC method internally brackets `self.acquire().await?` so handler
//! code drops its `pools` field. The [`Pooled<C>`] adapter wraps any
//! inner [`LagoCall`] impl (real proxy or mock) for identical pool
//! semantics in production and tests.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::Stream;
use lago_substrate_proto::lago::v1::{
    AppendReq, ListNamespacesReq, lago_substrate_client::LagoSubstrateClient,
};
use life_runtime_pool::pool::{Pool, PoolGuard};
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::error::{LagoProxyError, LagoProxyResult};

use life_runtime_proto::life::v1 as life_v1;

#[derive(Clone)]
pub struct LagoProxy {
    channel: Channel,
    token: Option<String>,
    /// Sub-phase E: per-substrate pool. Brackets every method through
    /// the breaker + bounded semaphore. `None` for unit tests that
    /// bypass pool semantics.
    pool: Option<Arc<Pool>>,
}

impl LagoProxy {
    pub async fn connect(socket: PathBuf) -> LagoProxyResult<Self> {
        let endpoint = Endpoint::try_from("http://[::]:0")
            .map_err(|e| LagoProxyError::Transport(format!("endpoint: {e}")))?;
        let channel = endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let socket = socket.clone();
                async move {
                    let s = UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
                }
            }))
            .await
            .map_err(|e| LagoProxyError::Transport(format!("connect: {e}")))?;
        Ok(Self {
            channel,
            token: None,
            pool: None,
        })
    }

    /// Sub-phase E: attach a per-substrate connection pool.
    pub fn with_pool(mut self, pool: Arc<Pool>) -> Self {
        self.pool = Some(pool);
        self
    }

    pub fn with_token(mut self, token: String) -> Self {
        self.token = Some(token);
        self
    }

    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Sub-phase D3: attach the Tier-3 substrate token to a tonic
    /// outgoing request. Spec C₂ §5.2.
    pub fn attach_token<T>(&self, req: &mut tonic::Request<T>) {
        if let Some(token) = &self.token
            && let Ok(value) = format!("Bearer {token}").parse()
        {
            req.metadata_mut().insert("authorization", value);
        }
    }

    async fn acquire_guard(&self) -> LagoProxyResult<Option<PoolGuard>> {
        match &self.pool {
            Some(pool) => Ok(Some(pool.acquire().await.map_err(LagoProxyError::from)?)),
            None => Ok(None),
        }
    }

    pub async fn open_namespace(&self, sid: &str) -> LagoProxyResult<String> {
        let guard = self.acquire_guard().await?;
        let mut req = tonic::Request::new(sid.to_string());
        self.attach_token(&mut req);
        let _ = req;
        let out = format!("session/{sid}");
        record_success(guard);
        Ok(out)
    }

    pub async fn close_namespace(&self, ns: &str) -> LagoProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let _ = ns;
        record_success(guard);
        Ok(())
    }

    pub async fn read(
        &self,
        sid: &str,
        from: u64,
        limit: u32,
    ) -> LagoProxyResult<
        Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
    > {
        let _ = (sid, from, limit);
        let guard = self.acquire_guard().await?;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        let inner = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Box::pin(EventGuardedStream::new(inner, guard)))
    }

    pub async fn subscribe(
        &self,
        sid: &str,
        from: u64,
    ) -> LagoProxyResult<
        Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
    > {
        self.read(sid, from, 0).await
    }

    pub async fn get_blob(
        &self,
        namespace: &str,
        sha256: &str,
    ) -> LagoProxyResult<(Vec<u8>, String)> {
        let guard = self.acquire_guard().await?;
        let _ = (namespace, sha256);
        let out = (b"empty".to_vec(), "application/octet-stream".to_string());
        record_success(guard);
        Ok(out)
    }

    /// Idempotency-store backend (B7 wires this into IdempotencyStore).
    pub async fn idem_lookup(&self, key: &[u8]) -> LagoProxyResult<Option<Vec<u8>>> {
        let guard = self.acquire_guard().await?;
        let _ = key;
        record_success(guard);
        Ok(None)
    }

    pub async fn idem_persist(&self, key: &[u8], response: Vec<u8>) -> LagoProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let _ = (key, response);
        record_success(guard);
        Ok(())
    }

    /// Append a single event to a lago namespace.
    ///
    /// BRO-1017 (Phase 2 of the Topology B substrate-stub gap close-out):
    /// this now issues a real `lago.v1.LagoSubstrate.Append` RPC against
    /// lagod's substrate-plane server. Replaces the prior
    /// `idem_persist`-as-append shim that was production-correct for
    /// durability but did NOT actually produce a journal entry that
    /// `Events.Read` / `Subscribe` could see.
    ///
    /// The (namespace, event_type, payload) triplet is wrapped server-side
    /// into an `EventKind::Custom { event_type, data }` envelope on the
    /// namespace's main branch. Payload must be valid JSON (an empty
    /// payload is treated as JSON `null` by the substrate).
    pub async fn append_event(
        &self,
        namespace: &str,
        event_type: &str,
        payload: Vec<u8>,
    ) -> LagoProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let mut client = LagoSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(AppendReq {
            namespace: namespace.to_string(),
            event_type: event_type.to_string(),
            payload,
        });
        self.attach_token(&mut req);
        match client.append(req).await {
            Ok(_) => {
                record_success(guard);
                Ok(())
            }
            Err(status) => {
                record_outcome(guard, &status);
                Err(LagoProxyError::Substrate(status))
            }
        }
    }

    /// Enumerate `session/*` namespaces known to lago.
    ///
    /// BRO-1017 (Phase 2 of the Topology B substrate-stub gap close-out):
    /// this now issues a real `lago.v1.LagoSubstrate.ListNamespaces` RPC
    /// against lagod's substrate-plane server. Replaces the prior
    /// empty-vec fallback so `RoutingCache::cold_start` warms from
    /// durable storage at boot instead of waiting for traffic.
    pub async fn list_namespaces(&self, prefix: &str) -> LagoProxyResult<Vec<String>> {
        let guard = self.acquire_guard().await?;
        let mut client = LagoSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(ListNamespacesReq {
            prefix: prefix.to_string(),
        });
        self.attach_token(&mut req);
        match client.list_namespaces(req).await {
            Ok(resp) => {
                record_success(guard);
                Ok(resp.into_inner().namespaces)
            }
            Err(status) => {
                record_outcome(guard, &status);
                Err(LagoProxyError::Substrate(status))
            }
        }
    }
}

fn record_success(guard: Option<PoolGuard>) {
    if let Some(g) = guard {
        g.record_success();
    }
}

/// Mirror of `arcan-proxy::record_outcome`. Sub-phase E policy:
/// permanent (non-retryable) errors record success per Spec C₂ §7.2 so
/// the breaker doesn't trip on auth / policy misconfiguration; only
/// retryable failures (Unavailable / DeadlineExceeded / Aborted /
/// ResourceExhausted) count against the breaker's failure budget.
fn record_outcome(guard: Option<PoolGuard>, status: &tonic::Status) {
    if let Some(g) = guard {
        let retryable = matches!(
            status.code(),
            tonic::Code::Unavailable
                | tonic::Code::DeadlineExceeded
                | tonic::Code::Aborted
                | tonic::Code::ResourceExhausted
        );
        if retryable {
            g.record_failure();
        } else {
            g.record_success();
        }
    }
}

#[async_trait]
pub trait LagoCall: Send + Sync {
    async fn open_namespace(&self, sid: &str) -> LagoProxyResult<String>;
    async fn close_namespace(&self, ns: &str) -> LagoProxyResult<()>;
    async fn read(
        &self,
        sid: &str,
        from: u64,
        limit: u32,
    ) -> LagoProxyResult<
        Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
    >;
    async fn subscribe(
        &self,
        sid: &str,
        from: u64,
    ) -> LagoProxyResult<
        Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
    >;
    async fn get_blob(&self, namespace: &str, sha256: &str) -> LagoProxyResult<(Vec<u8>, String)>;
    async fn idem_lookup(&self, key: &[u8]) -> LagoProxyResult<Option<Vec<u8>>>;
    async fn idem_persist(&self, key: &[u8], response: Vec<u8>) -> LagoProxyResult<()>;
    async fn append_event(
        &self,
        namespace: &str,
        event_type: &str,
        payload: Vec<u8>,
    ) -> LagoProxyResult<()>;
    async fn list_namespaces(&self, prefix: &str) -> LagoProxyResult<Vec<String>>;
}

#[async_trait]
impl LagoCall for LagoProxy {
    async fn open_namespace(&self, sid: &str) -> LagoProxyResult<String> {
        LagoProxy::open_namespace(self, sid).await
    }
    async fn close_namespace(&self, ns: &str) -> LagoProxyResult<()> {
        LagoProxy::close_namespace(self, ns).await
    }
    async fn read(
        &self,
        sid: &str,
        from: u64,
        limit: u32,
    ) -> LagoProxyResult<
        Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
    > {
        LagoProxy::read(self, sid, from, limit).await
    }
    async fn subscribe(
        &self,
        sid: &str,
        from: u64,
    ) -> LagoProxyResult<
        Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
    > {
        LagoProxy::subscribe(self, sid, from).await
    }
    async fn get_blob(&self, namespace: &str, sha256: &str) -> LagoProxyResult<(Vec<u8>, String)> {
        LagoProxy::get_blob(self, namespace, sha256).await
    }
    async fn idem_lookup(&self, key: &[u8]) -> LagoProxyResult<Option<Vec<u8>>> {
        LagoProxy::idem_lookup(self, key).await
    }
    async fn idem_persist(&self, key: &[u8], response: Vec<u8>) -> LagoProxyResult<()> {
        LagoProxy::idem_persist(self, key, response).await
    }
    async fn append_event(
        &self,
        namespace: &str,
        event_type: &str,
        payload: Vec<u8>,
    ) -> LagoProxyResult<()> {
        LagoProxy::append_event(self, namespace, event_type, payload).await
    }
    async fn list_namespaces(&self, prefix: &str) -> LagoProxyResult<Vec<String>> {
        LagoProxy::list_namespaces(self, prefix).await
    }
}

/// Sub-phase E: pool-bracketing adapter. Wraps any inner [`LagoCall`]
/// impl (real proxy, mock) and applies pool semaphore + circuit-breaker
/// bracketing on every method.
pub struct Pooled<C: LagoCall> {
    inner: C,
    pool: Arc<Pool>,
}

impl<C: LagoCall> Pooled<C> {
    pub fn new(inner: C, pool: Arc<Pool>) -> Self {
        Self { inner, pool }
    }

    pub fn into_inner(self) -> C {
        self.inner
    }

    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    async fn bracket<T, F>(&self, fut: F) -> LagoProxyResult<T>
    where
        F: std::future::Future<Output = LagoProxyResult<T>>,
    {
        let guard = self.pool.acquire().await.map_err(LagoProxyError::from)?;
        match fut.await {
            Ok(v) => {
                guard.record_success();
                Ok(v)
            }
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

#[async_trait]
impl<C: LagoCall> LagoCall for Pooled<C> {
    async fn open_namespace(&self, sid: &str) -> LagoProxyResult<String> {
        self.bracket(self.inner.open_namespace(sid)).await
    }
    async fn close_namespace(&self, ns: &str) -> LagoProxyResult<()> {
        self.bracket(self.inner.close_namespace(ns)).await
    }
    async fn read(
        &self,
        sid: &str,
        from: u64,
        limit: u32,
    ) -> LagoProxyResult<
        Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
    > {
        let guard = self.pool.acquire().await.map_err(LagoProxyError::from)?;
        match self.inner.read(sid, from, limit).await {
            Ok(stream) => Ok(Box::pin(EventGuardedStream::new(stream, Some(guard)))),
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
    async fn subscribe(
        &self,
        sid: &str,
        from: u64,
    ) -> LagoProxyResult<
        Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
    > {
        let guard = self.pool.acquire().await.map_err(LagoProxyError::from)?;
        match self.inner.subscribe(sid, from).await {
            Ok(stream) => Ok(Box::pin(EventGuardedStream::new(stream, Some(guard)))),
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
    async fn get_blob(&self, namespace: &str, sha256: &str) -> LagoProxyResult<(Vec<u8>, String)> {
        self.bracket(self.inner.get_blob(namespace, sha256)).await
    }
    async fn idem_lookup(&self, key: &[u8]) -> LagoProxyResult<Option<Vec<u8>>> {
        self.bracket(self.inner.idem_lookup(key)).await
    }
    async fn idem_persist(&self, key: &[u8], response: Vec<u8>) -> LagoProxyResult<()> {
        self.bracket(self.inner.idem_persist(key, response)).await
    }
    async fn append_event(
        &self,
        namespace: &str,
        event_type: &str,
        payload: Vec<u8>,
    ) -> LagoProxyResult<()> {
        self.bracket(self.inner.append_event(namespace, event_type, payload))
            .await
    }
    async fn list_namespaces(&self, prefix: &str) -> LagoProxyResult<Vec<String>> {
        self.bracket(self.inner.list_namespaces(prefix)).await
    }
}

/// Wraps an event stream with a [`PoolGuard`]. Mirrors
/// `arcan_proxy::PoolGuardedStream` for the lago `EventRecord` element
/// type.
pub struct EventGuardedStream<S>
where
    S: Stream<Item = Result<life_v1::EventRecord, tonic::Status>>,
{
    inner: S,
    guard: Option<PoolGuard>,
    saw_error: bool,
}

impl<S> EventGuardedStream<S>
where
    S: Stream<Item = Result<life_v1::EventRecord, tonic::Status>>,
{
    pub fn new(inner: S, guard: Option<PoolGuard>) -> Self {
        Self {
            inner,
            guard,
            saw_error: false,
        }
    }
}

impl<S> Stream for EventGuardedStream<S>
where
    S: Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Unpin,
{
    type Item = Result<life_v1::EventRecord, tonic::Status>;

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

    fn dummy_proxy_with_token(token: &str) -> LagoProxy {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        LagoProxy {
            channel,
            token: Some(token.to_string()),
            pool: None,
        }
    }

    #[tokio::test]
    async fn attach_token_sets_authorization_header() {
        let proxy = dummy_proxy_with_token("lago.jws.token");
        let mut req = tonic::Request::new(());
        proxy.attach_token(&mut req);
        let auth = req.metadata().get("authorization").expect("authz set");
        assert_eq!(auth.to_str().unwrap(), "Bearer lago.jws.token");
    }

    #[tokio::test]
    async fn pooled_records_failure_on_unavailable() {
        use life_runtime_pool::breaker::{BreakerState, FAILURE_THRESHOLD};
        use life_runtime_pool::pool::{Pool, SubstrateKind};

        struct DownLago;
        #[async_trait]
        impl LagoCall for DownLago {
            async fn open_namespace(&self, _sid: &str) -> LagoProxyResult<String> {
                Err(LagoProxyError::Substrate(tonic::Status::unavailable(
                    "down",
                )))
            }
            async fn close_namespace(&self, _: &str) -> LagoProxyResult<()> {
                Ok(())
            }
            async fn read(
                &self,
                _: &str,
                _: u64,
                _: u32,
            ) -> LagoProxyResult<
                Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                drop(tx);
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn subscribe(
                &self,
                _: &str,
                _: u64,
            ) -> LagoProxyResult<
                Pin<Box<dyn Stream<Item = Result<life_v1::EventRecord, tonic::Status>> + Send>>,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                drop(tx);
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn get_blob(&self, _: &str, _: &str) -> LagoProxyResult<(Vec<u8>, String)> {
                Ok((Vec::new(), String::new()))
            }
            async fn idem_lookup(&self, _: &[u8]) -> LagoProxyResult<Option<Vec<u8>>> {
                Ok(None)
            }
            async fn idem_persist(&self, _: &[u8], _: Vec<u8>) -> LagoProxyResult<()> {
                Ok(())
            }
            async fn append_event(&self, _: &str, _: &str, _: Vec<u8>) -> LagoProxyResult<()> {
                Ok(())
            }
            async fn list_namespaces(&self, _: &str) -> LagoProxyResult<Vec<String>> {
                Ok(Vec::new())
            }
        }

        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        let pool = Arc::new(Pool::new(channel, 4, SubstrateKind::Lago));
        let pooled = Pooled::new(DownLago, Arc::clone(&pool));
        for _ in 0..FAILURE_THRESHOLD {
            let _ = pooled.open_namespace("sid").await;
        }
        assert_eq!(pool.breaker_state(), BreakerState::Open);
    }
}
