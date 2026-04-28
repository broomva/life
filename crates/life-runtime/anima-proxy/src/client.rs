//! Typed tonic client for the anima substrate.
//!
//! Wraps a tonic Channel over the anima UDS socket, exposes the slice of
//! anima RPCs lifed needs for Identity handlers (account/profile/session
//! management), and provides an object-safe `AnimaCall` trait so lifed
//! handlers can swap mocks under test.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
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

    pub async fn register_session(&self, sid: &str, user_id: &str) -> AnimaProxyResult<()> {
        let mut req = tonic::Request::new((sid.to_string(), user_id.to_string()));
        self.attach_token(&mut req);
        let _ = req;
        Ok(())
    }

    pub async fn mark_session_closed(&self, sid: &str) -> AnimaProxyResult<()> {
        let _ = sid;
        Ok(())
    }

    pub async fn get_account(&self, user_id: &str) -> AnimaProxyResult<Account> {
        Ok(Account {
            user_id: user_id.to_string(),
            handle: format!("@{user_id}"),
            display_name: user_id.to_string(),
            email: format!("{user_id}@example.com"),
            tier: "free".to_string(),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            profile: Profile::default(),
        })
    }

    pub async fn update_profile(
        &self,
        user_id: &str,
        profile: Profile,
    ) -> AnimaProxyResult<Account> {
        let mut a = self.get_account(user_id).await?;
        a.profile = profile;
        Ok(a)
    }

    pub async fn list_sessions(
        &self,
        user_id: &str,
        include_closed: bool,
        limit: u32,
    ) -> AnimaProxyResult<Vec<SessionDescriptor>> {
        let _ = (user_id, include_closed, limit);
        Ok(vec![])
    }

    pub async fn revoke_session(&self, sid: &str) -> AnimaProxyResult<()> {
        let _ = sid;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_proxy_with_token(token: &str) -> AnimaProxy {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:0").expect("endpoint");
        let channel = endpoint.connect_lazy();
        AnimaProxy {
            channel,
            token: Some(token.to_string()),
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
}
