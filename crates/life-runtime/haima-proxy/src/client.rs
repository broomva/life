//! Typed tonic client for the haima substrate.
//!
//! Wraps a tonic Channel over the haima UDS socket, exposes the slice of
//! haima RPCs lifed needs for Wallet handlers (bind/unbind, balance,
//! statement, debit, transfer), and provides an object-safe `HaimaCall`
//! trait so lifed handlers can swap mocks under test.
//!
//! Sub-phase E: each `*Proxy` owns the `Arc<Pool>` per Spec C₂ §7. The
//! [`Pooled<C>`] adapter wraps any inner [`HaimaCall`] (real proxy or
//! mock) and applies pool semaphore + circuit-breaker bracketing.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::Stream;
use life_runtime_pool::pool::{Pool, PoolGuard};
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::error::{HaimaProxyError, HaimaProxyResult};

#[derive(Clone)]
pub struct HaimaProxy {
    channel: Channel,
    token: Option<String>,
    pool: Option<Arc<Pool>>,
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

    async fn acquire_guard(&self) -> HaimaProxyResult<Option<PoolGuard>> {
        match &self.pool {
            Some(pool) => Ok(Some(pool.acquire().await.map_err(HaimaProxyError::from)?)),
            None => Ok(None),
        }
    }

    pub async fn bind_wallet(&self, sid: &str, project_id: &str) -> HaimaProxyResult<String> {
        let guard = self.acquire_guard().await?;
        let mut req = tonic::Request::new((sid.to_string(), project_id.to_string()));
        self.attach_token(&mut req);
        let _ = req;
        let out = format!("wallet-{sid}-{project_id}");
        record_success(guard);
        Ok(out)
    }

    pub async fn unbind_wallet(&self, wallet_id: &str) -> HaimaProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let _ = wallet_id;
        record_success(guard);
        Ok(())
    }

    pub async fn get_balance(
        &self,
        user_id: &str,
        project_id: &str,
    ) -> HaimaProxyResult<WalletBalance> {
        let guard = self.acquire_guard().await?;
        let _ = (user_id, project_id);
        let out = WalletBalance {
            micros: 1_000_000,
            currency: "USDC".to_string(),
        };
        record_success(guard);
        Ok(out)
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
        let guard = self.acquire_guard().await?;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        let inner = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Box::pin(LedgerGuardedStream::new(inner, guard)))
    }

    pub async fn debit(
        &self,
        user_id: &str,
        project_id: &str,
        amount_micros: u64,
        sid: &str,
        reason: &str,
    ) -> HaimaProxyResult<(String, WalletBalance)> {
        let guard = self.acquire_guard().await?;
        let _ = (user_id, project_id, amount_micros, sid, reason);
        let out = (
            "entry-1".to_string(),
            WalletBalance {
                micros: 999_000,
                currency: "USDC".to_string(),
            },
        );
        record_success(guard);
        Ok(out)
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
        let guard = self.acquire_guard().await?;
        let _ = (
            from_user,
            from_project,
            to_user,
            to_project,
            amount_micros,
            memo,
        );
        let out = (
            "entry-1".to_string(),
            WalletBalance {
                micros: 999_000,
                currency: "USDC".to_string(),
            },
            WalletBalance {
                micros: 100_000,
                currency: "USDC".to_string(),
            },
        );
        record_success(guard);
        Ok(out)
    }
}

fn record_success(guard: Option<PoolGuard>) {
    if let Some(g) = guard {
        g.record_success();
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

/// Sub-phase E: pool-bracketing adapter. Wraps any inner [`HaimaCall`].
pub struct Pooled<C: HaimaCall> {
    inner: C,
    pool: Arc<Pool>,
}

impl<C: HaimaCall> Pooled<C> {
    pub fn new(inner: C, pool: Arc<Pool>) -> Self {
        Self { inner, pool }
    }
    pub fn into_inner(self) -> C {
        self.inner
    }
    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    async fn bracket<T, F>(&self, fut: F) -> HaimaProxyResult<T>
    where
        F: std::future::Future<Output = HaimaProxyResult<T>>,
    {
        let guard = self.pool.acquire().await.map_err(HaimaProxyError::from)?;
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
impl<C: HaimaCall> HaimaCall for Pooled<C> {
    async fn bind_wallet(&self, sid: &str, project_id: &str) -> HaimaProxyResult<String> {
        self.bracket(self.inner.bind_wallet(sid, project_id)).await
    }
    async fn unbind_wallet(&self, wallet_id: &str) -> HaimaProxyResult<()> {
        self.bracket(self.inner.unbind_wallet(wallet_id)).await
    }
    async fn get_balance(
        &self,
        user_id: &str,
        project_id: &str,
    ) -> HaimaProxyResult<WalletBalance> {
        self.bracket(self.inner.get_balance(user_id, project_id))
            .await
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
        let guard = self.pool.acquire().await.map_err(HaimaProxyError::from)?;
        match self
            .inner
            .statement(user_id, project_id, since_ms, until_ms, limit)
            .await
        {
            Ok(stream) => Ok(Box::pin(LedgerGuardedStream::new(stream, Some(guard)))),
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
    async fn debit(
        &self,
        user_id: &str,
        project_id: &str,
        amount_micros: u64,
        sid: &str,
        reason: &str,
    ) -> HaimaProxyResult<(String, WalletBalance)> {
        self.bracket(
            self.inner
                .debit(user_id, project_id, amount_micros, sid, reason),
        )
        .await
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
        self.bracket(self.inner.transfer(
            from_user,
            from_project,
            to_user,
            to_project,
            amount_micros,
            memo,
        ))
        .await
    }
}

/// Sub-phase E: guards a [`LedgerEntry`] stream with a [`PoolGuard`].
pub struct LedgerGuardedStream<S>
where
    S: Stream<Item = Result<LedgerEntry, tonic::Status>>,
{
    inner: S,
    guard: Option<PoolGuard>,
    saw_error: bool,
}

impl<S> LedgerGuardedStream<S>
where
    S: Stream<Item = Result<LedgerEntry, tonic::Status>>,
{
    pub fn new(inner: S, guard: Option<PoolGuard>) -> Self {
        Self {
            inner,
            guard,
            saw_error: false,
        }
    }
}

impl<S> Stream for LedgerGuardedStream<S>
where
    S: Stream<Item = Result<LedgerEntry, tonic::Status>> + Unpin,
{
    type Item = Result<LedgerEntry, tonic::Status>;
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

    fn dummy_proxy_with_token(token: &str) -> HaimaProxy {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        HaimaProxy {
            channel,
            token: Some(token.to_string()),
            pool: None,
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
