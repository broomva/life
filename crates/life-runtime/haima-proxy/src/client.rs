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
//!
//! BRO-1018 (Phase 3 of the Topology B substrate-stub gap close):
//! every `HaimaProxy::<rpc>` method below now issues a real
//! `haima.v1.WalletSubstrate` tonic call instead of returning a
//! hardcoded shape. The `Pooled<C>` adapter and the `LedgerGuardedStream`
//! wrapper are unchanged — they bracket whichever inner `HaimaCall`
//! the caller hands them, so mocks compose identically.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use aios_proto::aios::v1 as aios_v1;
use async_trait::async_trait;
use futures::Stream;
use haima_substrate_proto::haima::v1::{
    self as haima_pb, wallet_substrate_client::WalletSubstrateClient,
};
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

/// Outcome of an [`HaimaCall::x402_pay`] round-trip. Flattens
/// haima-x402's `X402PayResult` onto a single struct; `status` is the
/// discriminant (`"settled"` / `"not_required"` / `"declined"`) and the
/// remaining fields are populated per-variant. BRO-1354.
#[derive(Clone, Debug)]
pub struct X402PayOutcome {
    pub status: String,
    pub tx_hash: String,
    pub network: String,
    pub recipient: String,
    pub micro_credits: i64,
    pub declined_reason: String,
    pub settled: bool,
    pub resource_body: Vec<u8>,
    pub resource_status: u32,
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

    /// Bind a wallet to a session. BRO-1018 wires this to the real
    /// `haima.v1.WalletSubstrate.BindWallet` RPC. The substrate is
    /// idempotent on `(sid, project_id)`, so re-issuing the call
    /// after a saga retry is safe.
    pub async fn bind_wallet(&self, sid: &str, project_id: &str) -> HaimaProxyResult<String> {
        let guard = self.acquire_guard().await?;
        let mut client = WalletSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(haima_pb::BindWalletReq {
            sid: Some(aios_v1::SessionId {
                value: sid.to_owned(),
            }),
            project_id: project_id.to_owned(),
        });
        self.attach_token(&mut req);
        match client.bind_wallet(req).await.map_err(HaimaProxyError::from) {
            Ok(resp) => {
                let wallet_id = resp.into_inner().wallet_id;
                record_outcome(guard, true);
                Ok(wallet_id)
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
    }

    /// Unbind a wallet. BRO-1018 wires this to
    /// `haima.v1.WalletSubstrate.UnbindWallet`. Idempotent —
    /// wallets that don't exist substrate-side return Ok(empty).
    pub async fn unbind_wallet(&self, wallet_id: &str) -> HaimaProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let mut client = WalletSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(haima_pb::UnbindWalletReq {
            wallet_id: wallet_id.to_owned(),
        });
        self.attach_token(&mut req);
        match client
            .unbind_wallet(req)
            .await
            .map_err(HaimaProxyError::from)
        {
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

    pub async fn get_balance(
        &self,
        user_id: &str,
        project_id: &str,
    ) -> HaimaProxyResult<WalletBalance> {
        let guard = self.acquire_guard().await?;
        let mut client = WalletSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(haima_pb::GetBalanceReq {
            user_id: user_id.to_owned(),
            project_id: project_id.to_owned(),
        });
        self.attach_token(&mut req);
        match client.get_balance(req).await.map_err(HaimaProxyError::from) {
            Ok(resp) => {
                let body = resp.into_inner();
                record_outcome(guard, true);
                Ok(WalletBalance {
                    micros: body.micros,
                    currency: body.currency,
                })
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
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
        let guard = self.acquire_guard().await?;
        let mut client = WalletSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(haima_pb::StatementReq {
            user_id: user_id.to_owned(),
            project_id: project_id.to_owned(),
            since_ms,
            until_ms,
            limit,
        });
        self.attach_token(&mut req);
        let upstream = match client.statement(req).await {
            Ok(resp) => resp.into_inner(),
            Err(s) => {
                let err = HaimaProxyError::from(s);
                record_outcome(guard, !err.is_retryable());
                return Err(err);
            }
        };
        // Map haima.v1.LedgerEntry → public-plane LedgerEntry at the
        // wire boundary. The struct shapes are identical except for
        // the wallet_id field (Phase-3 server-side returns wallet_id
        // but the public-plane LedgerEntry has no slot for it — lifed's
        // public-plane Wallet.Statement already drops it via the
        // service mapping in `services/wallet.rs`).
        use futures::StreamExt;
        let mapped = upstream.map(|res| {
            res.map(|e| LedgerEntry {
                entry_id: e.entry_id,
                at_unix_ms: e.at_unix_ms,
                delta_micros: e.delta_micros,
                reason: e.reason,
                sid: e.sid,
            })
        });
        let inner = Box::pin(mapped);
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
        let mut client = WalletSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(haima_pb::DebitReq {
            user_id: user_id.to_owned(),
            project_id: project_id.to_owned(),
            amount_micros,
            sid: sid.to_owned(),
            reason: reason.to_owned(),
        });
        self.attach_token(&mut req);
        match client.debit(req).await.map_err(HaimaProxyError::from) {
            Ok(resp) => {
                let body = resp.into_inner();
                let bal = body.new_balance.ok_or_else(|| {
                    HaimaProxyError::InvalidResponse("debit: missing new_balance".to_string())
                })?;
                record_outcome(guard, true);
                Ok((
                    body.entry_id,
                    WalletBalance {
                        micros: bal.micros,
                        currency: bal.currency,
                    },
                ))
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
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
        let mut client = WalletSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(haima_pb::TransferReq {
            from_user: from_user.to_owned(),
            from_project: from_project.to_owned(),
            to_user: to_user.to_owned(),
            to_project: to_project.to_owned(),
            amount_micros,
            memo: memo.to_owned(),
        });
        self.attach_token(&mut req);
        match client.transfer(req).await.map_err(HaimaProxyError::from) {
            Ok(resp) => {
                let body = resp.into_inner();
                let from_bal = body.from_balance.ok_or_else(|| {
                    HaimaProxyError::InvalidResponse("transfer: missing from_balance".to_string())
                })?;
                let to_bal = body.to_balance.ok_or_else(|| {
                    HaimaProxyError::InvalidResponse("transfer: missing to_balance".to_string())
                })?;
                record_outcome(guard, true);
                Ok((
                    body.entry_id,
                    WalletBalance {
                        micros: from_bal.micros,
                        currency: from_bal.currency,
                    },
                    WalletBalance {
                        micros: to_bal.micros,
                        currency: to_bal.currency,
                    },
                ))
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
    }

    /// Initiate an x402 payment via `haima.v1.WalletSubstrate.X402Pay`.
    /// BRO-1354. The substrate signs from the user's Anima-custodied
    /// wallet and drives the full client round-trip; this proxy just
    /// marshals the request + maps the flat response.
    pub async fn x402_pay(
        &self,
        user_id: &str,
        project_id: &str,
        resource_url: &str,
        network: &str,
        max_amount_micros: Option<i64>,
    ) -> HaimaProxyResult<X402PayOutcome> {
        let guard = self.acquire_guard().await?;
        let mut client = WalletSubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(haima_pb::X402PayReq {
            user_id: user_id.to_owned(),
            project_id: project_id.to_owned(),
            resource_url: resource_url.to_owned(),
            network: network.to_owned(),
            max_amount_micros,
        });
        self.attach_token(&mut req);
        match client.x402_pay(req).await.map_err(HaimaProxyError::from) {
            Ok(resp) => {
                let body = resp.into_inner();
                record_outcome(guard, true);
                Ok(X402PayOutcome {
                    status: body.status,
                    tx_hash: body.tx_hash,
                    network: body.network,
                    recipient: body.recipient,
                    micro_credits: body.micro_credits,
                    declined_reason: body.declined_reason,
                    settled: body.settled,
                    resource_body: body.resource_body,
                    resource_status: body.resource_status,
                })
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
    }
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
    async fn x402_pay(
        &self,
        user_id: &str,
        project_id: &str,
        resource_url: &str,
        network: &str,
        max_amount_micros: Option<i64>,
    ) -> HaimaProxyResult<X402PayOutcome>;
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
    async fn x402_pay(
        &self,
        user_id: &str,
        project_id: &str,
        resource_url: &str,
        network: &str,
        max_amount_micros: Option<i64>,
    ) -> HaimaProxyResult<X402PayOutcome> {
        HaimaProxy::x402_pay(
            self,
            user_id,
            project_id,
            resource_url,
            network,
            max_amount_micros,
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
    async fn x402_pay(
        &self,
        user_id: &str,
        project_id: &str,
        resource_url: &str,
        network: &str,
        max_amount_micros: Option<i64>,
    ) -> HaimaProxyResult<X402PayOutcome> {
        self.bracket(self.inner.x402_pay(
            user_id,
            project_id,
            resource_url,
            network,
            max_amount_micros,
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

    #[tokio::test]
    async fn record_outcome_no_op_when_guard_absent() {
        // Compile-only smoke: the helper accepts None and does not panic.
        record_outcome(None, true);
        record_outcome(None, false);
    }
}
