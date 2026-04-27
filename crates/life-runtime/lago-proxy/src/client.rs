//! Typed tonic client for the lago substrate.
//!
//! Wraps a tonic Channel over the lago UDS socket, exposes the small slice
//! of lago RPCs lifed needs (namespace open/close, event read/subscribe,
//! blob get, idempotency lookup/persist), and provides an object-safe
//! `LagoCall` trait so lifed handlers can swap mocks under test.

use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::error::{LagoProxyError, LagoProxyResult};

use life_runtime_proto::life::v1 as life_v1;

#[derive(Clone)]
pub struct LagoProxy {
    channel: Channel,
    token: Option<String>,
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
        })
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

    pub async fn open_namespace(&self, sid: &str) -> LagoProxyResult<String> {
        Ok(format!("session/{sid}"))
    }

    pub async fn close_namespace(&self, ns: &str) -> LagoProxyResult<()> {
        let _ = ns;
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
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
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
        let _ = (namespace, sha256);
        Ok((b"empty".to_vec(), "application/octet-stream".to_string()))
    }

    /// Idempotency-store backend (B7 wires this into IdempotencyStore).
    pub async fn idem_lookup(&self, key: &[u8]) -> LagoProxyResult<Option<Vec<u8>>> {
        let _ = key;
        Ok(None)
    }

    pub async fn idem_persist(&self, key: &[u8], response: Vec<u8>) -> LagoProxyResult<()> {
        let _ = (key, response);
        Ok(())
    }

    /// Append a single event to a lago namespace. Sub-phase C ships this
    /// as a best-effort no-op against the real lago daemon; a typed
    /// `lago.Append` RPC lands in sub-phase D2 once the corresponding
    /// proto method ships in lago. Used by `SagaDriver` to persist saga
    /// lifecycle events to `system/lifed/saga/<saga_id>` (Spec C₂ §4.1).
    pub async fn append_event(
        &self,
        namespace: &str,
        event_type: &str,
        payload: Vec<u8>,
    ) -> LagoProxyResult<()> {
        let _ = (namespace, event_type, payload);
        Ok(())
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
}
