//! Typed tonic client for the anima substrate.
//!
//! Wraps a tonic Channel over the anima UDS socket, exposes the slice of
//! anima RPCs lifed needs for Identity handlers (account/profile/session
//! management), and provides an object-safe `AnimaCall` trait so lifed
//! handlers can swap mocks under test.
//!
//! Sub-phase E: each `*Proxy` owns the `Arc<Pool>` per Spec C₂ §7. The
//! [`Pooled<C>`] adapter wraps any inner [`AnimaCall`] (real proxy or
//! mock) and applies pool semaphore + circuit-breaker bracketing.
//!
//! BRO-1019 (Phase 4 of the Topology B substrate-stub gap close):
//! every `AnimaProxy::<rpc>` method below now issues a real
//! `anima.v1.IdentitySubstrate` tonic call against soma's admin UDS
//! (which mounts both `CustodyOracle` and `IdentitySubstrate` per
//! Spec D D-Sub-E + BRO-1019). The `Pooled<C>` adapter is unchanged
//! — it brackets whichever inner `AnimaCall` the caller hands it, so
//! mocks compose identically.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aios_proto::aios::v1 as aios_v1;
use anima_substrate_proto::anima::v1::{
    self as anima_pb, identity_substrate_client::IdentitySubstrateClient,
};
use async_trait::async_trait;
use life_runtime_pool::pool::{Pool, PoolGuard};
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::error::{AnimaProxyError, AnimaProxyResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub user_id: String,
    pub handle: String,
    pub display_name: String,
    pub email: String,
    pub tier: String,
    pub created_at_ms: i64,
    pub profile: Profile,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Profile {
    pub bio: String,
    pub avatar_blob_ref: Vec<u8>,
    pub preferences: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionDescriptor {
    pub sid: String,
    pub project_id: String,
    pub opened_at_ms: i64,
    pub closed_at_ms: i64,
    pub label: String,
}

#[derive(Clone)]
pub struct AnimaProxy {
    channel: Channel,
    token: Option<String>,
    pool: Option<Arc<Pool>>,
}

impl AnimaProxy {
    pub async fn connect(socket: PathBuf) -> AnimaProxyResult<Self> {
        let endpoint = Endpoint::try_from("http://[::]:0")
            .map_err(|e| AnimaProxyError::Transport(format!("endpoint: {e}")))?;
        let channel = endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let socket = socket.clone();
                async move {
                    let s = UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
                }
            }))
            .await
            .map_err(|e| AnimaProxyError::Transport(format!("connect: {e}")))?;
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

    async fn acquire_guard(&self) -> AnimaProxyResult<Option<PoolGuard>> {
        match &self.pool {
            Some(pool) => Ok(Some(pool.acquire().await.map_err(AnimaProxyError::from)?)),
            None => Ok(None),
        }
    }

    /// Register a session substrate-side. BRO-1019 wires this to the
    /// real `anima.v1.IdentitySubstrate.RegisterSession` RPC. Idempotent
    /// on sid — re-issuing after a saga retry is safe.
    pub async fn register_session(&self, sid: &str, user_id: &str) -> AnimaProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let mut client = IdentitySubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(anima_pb::RegisterSessionReq {
            sid: Some(aios_v1::SessionId {
                value: sid.to_owned(),
            }),
            user_id: user_id.to_owned(),
        });
        self.attach_token(&mut req);
        match client
            .register_session(req)
            .await
            .map_err(AnimaProxyError::from)
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

    /// Mark a session closed. BRO-1019 wires this to
    /// `anima.v1.IdentitySubstrate.MarkSessionClosed`. Idempotent —
    /// unknown sids return Ok.
    pub async fn mark_session_closed(&self, sid: &str) -> AnimaProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let mut client = IdentitySubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(anima_pb::MarkSessionClosedReq {
            sid: Some(aios_v1::SessionId {
                value: sid.to_owned(),
            }),
        });
        self.attach_token(&mut req);
        match client
            .mark_session_closed(req)
            .await
            .map_err(AnimaProxyError::from)
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

    /// Fetch (or materialize) the account for a user. BRO-1019 wires
    /// this to `anima.v1.IdentitySubstrate.GetAccount`. The substrate
    /// creates a default account on first access (mirror of the
    /// proxy's pre-BRO-1019 fallback shape).
    pub async fn get_account(&self, user_id: &str) -> AnimaProxyResult<Account> {
        let guard = self.acquire_guard().await?;
        let mut client = IdentitySubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(anima_pb::GetAccountReq {
            user_id: user_id.to_owned(),
        });
        self.attach_token(&mut req);
        match client.get_account(req).await.map_err(AnimaProxyError::from) {
            Ok(resp) => {
                let body = resp.into_inner();
                record_outcome(guard, true);
                Ok(account_from_proto(body))
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
    }

    /// Replace the profile sub-record. BRO-1019 wires this to
    /// `anima.v1.IdentitySubstrate.UpdateProfile`. Returns the updated
    /// account.
    pub async fn update_profile(
        &self,
        user_id: &str,
        profile: Profile,
    ) -> AnimaProxyResult<Account> {
        let guard = self.acquire_guard().await?;
        let mut client = IdentitySubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(anima_pb::UpdateProfileReq {
            user_id: user_id.to_owned(),
            profile: Some(profile_to_proto(profile)),
        });
        self.attach_token(&mut req);
        match client
            .update_profile(req)
            .await
            .map_err(AnimaProxyError::from)
        {
            Ok(resp) => {
                let body = resp.into_inner();
                record_outcome(guard, true);
                Ok(account_from_proto(body))
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
    }

    /// List sessions for a user. BRO-1019 wires this to
    /// `anima.v1.IdentitySubstrate.ListSessions`. Phase 4 returns all
    /// matching sessions in one shot (no pagination — see proto comment).
    pub async fn list_sessions(
        &self,
        user_id: &str,
        include_closed: bool,
        limit: u32,
    ) -> AnimaProxyResult<Vec<SessionDescriptor>> {
        let guard = self.acquire_guard().await?;
        let mut client = IdentitySubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(anima_pb::ListSessionsReq {
            user_id: user_id.to_owned(),
            include_closed,
            limit,
        });
        self.attach_token(&mut req);
        match client
            .list_sessions(req)
            .await
            .map_err(AnimaProxyError::from)
        {
            Ok(resp) => {
                let body = resp.into_inner();
                record_outcome(guard, true);
                Ok(body.sessions.into_iter().map(session_from_proto).collect())
            }
            Err(e) => {
                record_outcome(guard, !e.is_retryable());
                Err(e)
            }
        }
    }

    /// Revoke a session. BRO-1019 wires this to
    /// `anima.v1.IdentitySubstrate.RevokeSession`. Idempotent.
    pub async fn revoke_session(&self, sid: &str) -> AnimaProxyResult<()> {
        let guard = self.acquire_guard().await?;
        let mut client = IdentitySubstrateClient::new(self.channel.clone());
        let mut req = tonic::Request::new(anima_pb::RevokeSessionReq {
            sid: Some(aios_v1::SessionId {
                value: sid.to_owned(),
            }),
        });
        self.attach_token(&mut req);
        match client
            .revoke_session(req)
            .await
            .map_err(AnimaProxyError::from)
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
}

/// Record a `PoolGuard` outcome based on whether the call succeeded
/// or whether the error is a non-retryable / permanent failure (the
/// breaker treats permanent faults as success — they aren't infra
/// problems — per Spec C₂ §7.2). Mirrors the policy applied by
/// `Pooled<C>::bracket` and `haima-proxy`'s `record_outcome` helper
/// (BRO-1018).
fn record_outcome(guard: Option<PoolGuard>, success_or_permanent: bool) {
    if let Some(g) = guard {
        if success_or_permanent {
            g.record_success();
        } else {
            g.record_failure();
        }
    }
}

fn account_from_proto(p: anima_pb::Account) -> Account {
    Account {
        user_id: p.user_id,
        handle: p.handle,
        display_name: p.display_name,
        email: p.email,
        tier: p.tier,
        created_at_ms: p.created_at_ms,
        profile: p.profile.map(profile_from_proto).unwrap_or_default(),
    }
}

fn profile_from_proto(p: anima_pb::Profile) -> Profile {
    Profile {
        bio: p.bio,
        avatar_blob_ref: p.avatar_blob_ref,
        preferences: p.preferences,
    }
}

fn profile_to_proto(p: Profile) -> anima_pb::Profile {
    anima_pb::Profile {
        bio: p.bio,
        avatar_blob_ref: p.avatar_blob_ref,
        preferences: p.preferences,
    }
}

fn session_from_proto(s: anima_pb::SessionDescriptor) -> SessionDescriptor {
    SessionDescriptor {
        sid: s.sid,
        project_id: s.project_id,
        opened_at_ms: s.opened_at_ms,
        closed_at_ms: s.closed_at_ms,
        label: s.label,
    }
}

#[async_trait]
pub trait AnimaCall: Send + Sync {
    async fn register_session(&self, sid: &str, user_id: &str) -> AnimaProxyResult<()>;
    async fn mark_session_closed(&self, sid: &str) -> AnimaProxyResult<()>;
    async fn get_account(&self, user_id: &str) -> AnimaProxyResult<Account>;
    async fn update_profile(&self, user_id: &str, profile: Profile) -> AnimaProxyResult<Account>;
    async fn list_sessions(
        &self,
        user_id: &str,
        include_closed: bool,
        limit: u32,
    ) -> AnimaProxyResult<Vec<SessionDescriptor>>;
    async fn revoke_session(&self, sid: &str) -> AnimaProxyResult<()>;
}

#[async_trait]
impl AnimaCall for AnimaProxy {
    async fn register_session(&self, sid: &str, user_id: &str) -> AnimaProxyResult<()> {
        AnimaProxy::register_session(self, sid, user_id).await
    }
    async fn mark_session_closed(&self, sid: &str) -> AnimaProxyResult<()> {
        AnimaProxy::mark_session_closed(self, sid).await
    }
    async fn get_account(&self, user_id: &str) -> AnimaProxyResult<Account> {
        AnimaProxy::get_account(self, user_id).await
    }
    async fn update_profile(&self, user_id: &str, profile: Profile) -> AnimaProxyResult<Account> {
        AnimaProxy::update_profile(self, user_id, profile).await
    }
    async fn list_sessions(
        &self,
        user_id: &str,
        include_closed: bool,
        limit: u32,
    ) -> AnimaProxyResult<Vec<SessionDescriptor>> {
        AnimaProxy::list_sessions(self, user_id, include_closed, limit).await
    }
    async fn revoke_session(&self, sid: &str) -> AnimaProxyResult<()> {
        AnimaProxy::revoke_session(self, sid).await
    }
}

/// Sub-phase E: pool-bracketing adapter. Wraps any inner [`AnimaCall`].
pub struct Pooled<C: AnimaCall> {
    inner: C,
    pool: Arc<Pool>,
}

impl<C: AnimaCall> Pooled<C> {
    pub fn new(inner: C, pool: Arc<Pool>) -> Self {
        Self { inner, pool }
    }
    pub fn into_inner(self) -> C {
        self.inner
    }
    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    async fn bracket<T, F>(&self, fut: F) -> AnimaProxyResult<T>
    where
        F: std::future::Future<Output = AnimaProxyResult<T>>,
    {
        let guard = self.pool.acquire().await.map_err(AnimaProxyError::from)?;
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
impl<C: AnimaCall> AnimaCall for Pooled<C> {
    async fn register_session(&self, sid: &str, user_id: &str) -> AnimaProxyResult<()> {
        self.bracket(self.inner.register_session(sid, user_id))
            .await
    }
    async fn mark_session_closed(&self, sid: &str) -> AnimaProxyResult<()> {
        self.bracket(self.inner.mark_session_closed(sid)).await
    }
    async fn get_account(&self, user_id: &str) -> AnimaProxyResult<Account> {
        self.bracket(self.inner.get_account(user_id)).await
    }
    async fn update_profile(&self, user_id: &str, profile: Profile) -> AnimaProxyResult<Account> {
        self.bracket(self.inner.update_profile(user_id, profile))
            .await
    }
    async fn list_sessions(
        &self,
        user_id: &str,
        include_closed: bool,
        limit: u32,
    ) -> AnimaProxyResult<Vec<SessionDescriptor>> {
        self.bracket(self.inner.list_sessions(user_id, include_closed, limit))
            .await
    }
    async fn revoke_session(&self, sid: &str) -> AnimaProxyResult<()> {
        self.bracket(self.inner.revoke_session(sid)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_proxy_with_token(token: &str) -> AnimaProxy {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        AnimaProxy {
            channel,
            token: Some(token.to_string()),
            pool: None,
        }
    }

    #[tokio::test]
    async fn attach_token_sets_authorization_header() {
        let proxy = dummy_proxy_with_token("anima.jws.token");
        let mut req = tonic::Request::new(());
        proxy.attach_token(&mut req);
        let auth = req.metadata().get("authorization").expect("authz set");
        assert_eq!(auth.to_str().unwrap(), "Bearer anima.jws.token");
    }

    #[tokio::test]
    async fn record_outcome_no_op_when_guard_absent() {
        // Compile-only smoke: the helper accepts None and does not panic.
        record_outcome(None, true);
        record_outcome(None, false);
    }
}
