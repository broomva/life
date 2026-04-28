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

use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::error::{ArcanProxyError, ArcanProxyResult};

#[derive(Clone)]
pub struct ArcanProxy {
    channel: Channel,
    token: Option<String>,
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
        })
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
    ///
    /// This is the canonical token-attachment helper used by every
    /// outgoing tonic call once the real arcan-proto ships. Until then
    /// the stub methods bypass tonic but still call this helper for
    /// shape parity with sub-phase E.
    pub fn attach_token<T>(&self, req: &mut tonic::Request<T>) {
        if let Some(token) = &self.token
            && let Ok(value) = format!("Bearer {token}").parse()
        {
            req.metadata_mut().insert("authorization", value);
        }
    }

    /// Stub: pretend to create an agent and return an opaque agent_id.
    /// Real impl invokes the arcan AgentService.CreateAgent RPC.
    pub async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
        // Sub-phase D3: even the stub path goes through the token
        // attachment helper so shape parity holds with sub-phase E's
        // tonic-client implementation.
        let mut req = tonic::Request::new(sid.to_string());
        self.attach_token(&mut req);
        let _ = (req, &self.channel);
        Ok(format!("agent-{sid}"))
    }

    pub async fn destroy_agent(&self, sid: &str) -> ArcanProxyResult<()> {
        let _ = (sid, &self.channel);
        Ok(())
    }

    pub async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
    ) -> ArcanProxyResult<
        Pin<
            Box<
                dyn Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>>
                    + Send,
            >,
        >,
    > {
        let _ = (sid, content);
        // Sub-phase B initial: re-use the same canned stream as the mock until
        // the arcan RPC is ready.
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(life_runtime_proto::life::v1::AgentEvent {
                    record: None,
                    kind: life_runtime_proto::life::v1::AgentEventKind::Token as i32,
                }))
                .await;
            let _ = tx
                .send(Ok(life_runtime_proto::life::v1::AgentEvent {
                    record: None,
                    kind: life_runtime_proto::life::v1::AgentEventKind::Finish as i32,
                }))
                .await;
        });
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

/// Object-safe trait covering the lifed-relevant subset of arcan operations.
/// Used in `lifed::services::agent` so the integration tests can swap the
/// real proxy for a mock under test.
#[async_trait]
pub trait ArcanCall: Send + Sync {
    async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String>;
    async fn destroy_agent(&self, sid: &str) -> ArcanProxyResult<()>;
    async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
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
    ) -> ArcanProxyResult<
        Pin<
            Box<
                dyn Stream<Item = Result<life_runtime_proto::life::v1::AgentEvent, tonic::Status>>
                    + Send,
            >,
        >,
    > {
        ArcanProxy::dispatch_message(self, sid, content).await
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
        };
        let mut req = tonic::Request::new(());
        proxy.attach_token(&mut req);
        assert!(req.metadata().get("authorization").is_none());
    }
}
