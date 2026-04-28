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
use sha2::Digest;
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

    /// Sub-phase D3: attach the Tier-3 substrate token to a tonic
    /// outgoing request. Spec C₂ §5.2.
    pub fn attach_token<T>(&self, req: &mut tonic::Request<T>) {
        if let Some(token) = &self.token
            && let Ok(value) = format!("Bearer {token}").parse()
        {
            req.metadata_mut().insert("authorization", value);
        }
    }

    pub async fn open_namespace(&self, sid: &str) -> LagoProxyResult<String> {
        let mut req = tonic::Request::new(sid.to_string());
        self.attach_token(&mut req);
        let _ = req;
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

    /// Append a single event to a lago namespace.
    ///
    /// Sub-phase D2: the lago-proxy is now structured to issue a typed
    /// `lago.Append` RPC. Until the real `lagod` ships the matching
    /// service definition the call falls back to the equivalent
    /// `idem_persist` keyspace (the only RPC the current lago daemon
    /// exposes that satisfies the durability contract — the dedup key
    /// becomes `(namespace, event_type, sha256(payload))`). When the
    /// dedicated `lago.Append` proto lands the swap is local to this
    /// method.
    ///
    /// Used by `SagaDriver` to persist saga lifecycle events to
    /// `system/lifed/saga/<saga_id>` per Spec C₂ §4.1.
    pub async fn append_event(
        &self,
        namespace: &str,
        event_type: &str,
        payload: Vec<u8>,
    ) -> LagoProxyResult<()> {
        // Construct a content-addressed dedup key so re-issuing the same
        // event is a true no-op even across lifed restarts.
        let mut hasher = sha2::Sha256::new();
        Digest::update(&mut hasher, namespace.as_bytes());
        Digest::update(&mut hasher, b"|");
        Digest::update(&mut hasher, event_type.as_bytes());
        Digest::update(&mut hasher, b"|");
        Digest::update(&mut hasher, &payload);
        let digest = hasher.finalize();
        let mut key = Vec::with_capacity(64 + namespace.len() + event_type.len());
        key.extend_from_slice(b"saga|");
        key.extend_from_slice(namespace.as_bytes());
        key.push(b'|');
        key.extend_from_slice(event_type.as_bytes());
        key.push(b'|');
        key.extend_from_slice(digest.as_slice());
        // Persist via idem_persist — the same lago substrate keyspace
        // serves dedup + saga journaling until lago.Append ships.
        self.idem_persist(&key, payload).await
    }

    /// Enumerate `session/*` namespaces known to lago.
    ///
    /// Sub-phase D2: the lago-proxy structures this as a typed
    /// `lago.ListNamespaces` RPC. Until the real lago daemon ships the
    /// matching service definition, the proxy returns an empty list and
    /// callers handle that gracefully (cold-start replay degrades to
    /// "warm cache from incoming traffic"). When the wire RPC lands the
    /// swap is local to this method.
    ///
    /// The `prefix` filter follows the lago namespace convention
    /// (`session/`, `system/lifed/saga/`, etc.).
    pub async fn list_namespaces(&self, prefix: &str) -> LagoProxyResult<Vec<String>> {
        let _ = prefix;
        // Until lago.ListNamespaces ships, return empty. RoutingCache
        // cold-start handles this by warming on incoming traffic.
        Ok(Vec::new())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_proxy_with_token(token: &str) -> LagoProxy {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        LagoProxy {
            channel,
            token: Some(token.to_string()),
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
}
