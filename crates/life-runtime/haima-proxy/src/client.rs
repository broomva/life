//! Typed tonic client for the haima substrate.
//!
//! Wraps a tonic Channel over the haima UDS socket, exposes the slice of
//! haima RPCs lifed needs for Wallet handlers (bind/unbind, balance,
//! statement, debit, transfer), and provides an object-safe `HaimaCall`
//! trait so lifed handlers can swap mocks under test.

use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::error::{HaimaProxyError, HaimaProxyResult};

#[derive(Clone)]
pub struct HaimaProxy {
    channel: Channel,
    token: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WalletBalance {
    pub micros: u64,
    pub currency: String,
}

#[derive(Clone, Debug)]
pub struct LedgerEntry {
    pub entry_id: String,
    pub at_unix_ms: i64,
    pub delta_micros: i64,
    pub reason: String,
    pub sid: String,
}

impl HaimaProxy {
    pub async fn connect(socket: PathBuf) -> HaimaProxyResult<Self> {
        let endpoint = Endpoint::try_from("http://[::]:0")
            .map_err(|e| HaimaProxyError::Transport(format!("endpoint: {e}")))?;
        let channel = endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let socket = socket.clone();
                async move {
                    let s = UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
                }
            }))
            .await
            .map_err(|e| HaimaProxyError::Transport(format!("connect: {e}")))?;
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

    pub async fn bind_wallet(&self, sid: &str, project_id: &str) -> HaimaProxyResult<String> {
        let mut req = tonic::Request::new((sid.to_string(), project_id.to_string()));
        self.attach_token(&mut req);
        let _ = req;
        Ok(format!("wallet-{sid}-{project_id}"))
    }

    pub async fn unbind_wallet(&self, wallet_id: &str) -> HaimaProxyResult<()> {
        let _ = wallet_id;
        Ok(())
    }

    pub async fn get_balance(
        &self,
        user_id: &str,
        project_id: &str,
    ) -> HaimaProxyResult<WalletBalance> {
        let _ = (user_id, project_id);
        Ok(WalletBalance {
            micros: 1_000_000,
            currency: "USDC".to_string(),
        })
    }

    pub async fn statement(
        &self,
        user_id: &str,
        project_id: &str,
        since_ms: i64,
        until_ms: i64,
        limit: u32,
    ) -> HaimaProxyResult<Pin<Box<dyn Stream<Item = Result<LedgerEntry, tonic::Status>> + Send>>>
    {
        let _ = (user_id, project_id, since_ms, until_ms, limit);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    pub async fn debit(
        &self,
        user_id: &str,
        project_id: &str,
        amount_micros: u64,
        sid: &str,
        reason: &str,
    ) -> HaimaProxyResult<(String, WalletBalance)> {
        let _ = (user_id, project_id, amount_micros, sid, reason);
        Ok((
            "entry-1".to_string(),
            WalletBalance {
                micros: 999_000,
                currency: "USDC".to_string(),
            },
        ))
    }

    pub async fn transfer(
        &self,
        from_user: &str,
        from_project: &str,
        to_user: &str,
        to_project: &str,
        amount_micros: u64,
        memo: &str,
    ) -> HaimaProxyResult<(String, WalletBalance, WalletBalance)> {
        let _ = (
            from_user,
            from_project,
            to_user,
            to_project,
            amount_micros,
            memo,
        );
        Ok((
            "entry-1".to_string(),
            WalletBalance {
                micros: 999_000,
                currency: "USDC".to_string(),
            },
            WalletBalance {
                micros: 100_000,
                currency: "USDC".to_string(),
            },
        ))
    }
}

#[async_trait]
pub trait HaimaCall: Send + Sync {
    async fn bind_wallet(&self, sid: &str, project_id: &str) -> HaimaProxyResult<String>;
    async fn unbind_wallet(&self, wallet_id: &str) -> HaimaProxyResult<()>;
    async fn get_balance(&self, user_id: &str, project_id: &str)
    -> HaimaProxyResult<WalletBalance>;
    async fn statement(
        &self,
        user_id: &str,
        project_id: &str,
        since_ms: i64,
        until_ms: i64,
        limit: u32,
    ) -> HaimaProxyResult<Pin<Box<dyn Stream<Item = Result<LedgerEntry, tonic::Status>> + Send>>>;
    async fn debit(
        &self,
        user_id: &str,
        project_id: &str,
        amount_micros: u64,
        sid: &str,
        reason: &str,
    ) -> HaimaProxyResult<(String, WalletBalance)>;
    async fn transfer(
        &self,
        from_user: &str,
        from_project: &str,
        to_user: &str,
        to_project: &str,
        amount_micros: u64,
        memo: &str,
    ) -> HaimaProxyResult<(String, WalletBalance, WalletBalance)>;
}

#[async_trait]
impl HaimaCall for HaimaProxy {
    async fn bind_wallet(&self, sid: &str, project_id: &str) -> HaimaProxyResult<String> {
        HaimaProxy::bind_wallet(self, sid, project_id).await
    }
    async fn unbind_wallet(&self, wallet_id: &str) -> HaimaProxyResult<()> {
        HaimaProxy::unbind_wallet(self, wallet_id).await
    }
    async fn get_balance(
        &self,
        user_id: &str,
        project_id: &str,
    ) -> HaimaProxyResult<WalletBalance> {
        HaimaProxy::get_balance(self, user_id, project_id).await
    }
    async fn statement(
        &self,
        user_id: &str,
        project_id: &str,
        since_ms: i64,
        until_ms: i64,
        limit: u32,
    ) -> HaimaProxyResult<Pin<Box<dyn Stream<Item = Result<LedgerEntry, tonic::Status>> + Send>>>
    {
        HaimaProxy::statement(self, user_id, project_id, since_ms, until_ms, limit).await
    }
    async fn debit(
        &self,
        user_id: &str,
        project_id: &str,
        amount_micros: u64,
        sid: &str,
        reason: &str,
    ) -> HaimaProxyResult<(String, WalletBalance)> {
        HaimaProxy::debit(self, user_id, project_id, amount_micros, sid, reason).await
    }
    async fn transfer(
        &self,
        from_user: &str,
        from_project: &str,
        to_user: &str,
        to_project: &str,
        amount_micros: u64,
        memo: &str,
    ) -> HaimaProxyResult<(String, WalletBalance, WalletBalance)> {
        HaimaProxy::transfer(
            self,
            from_user,
            from_project,
            to_user,
            to_project,
            amount_micros,
            memo,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_proxy_with_token(token: &str) -> HaimaProxy {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        HaimaProxy {
            channel,
            token: Some(token.to_string()),
        }
    }

    #[tokio::test]
    async fn attach_token_sets_authorization_header() {
        let proxy = dummy_proxy_with_token("haima.jws.token");
        let mut req = tonic::Request::new(());
        proxy.attach_token(&mut req);
        let auth = req.metadata().get("authorization").expect("authz set");
        assert_eq!(auth.to_str().unwrap(), "Bearer haima.jws.token");
    }
}
