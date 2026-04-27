//! Development mocks for arcan/lago/haima/anima substrates.
//!
//! Sub-phase B retains these as deterministic test fixtures; the production
//! daemon path uses the real `*-proxy` crates over UDS. Mocks implement the
//! per-substrate `*Call` traits directly so integration tests can swap them
//! into `AgentService` without bridge adapters.
//!
//! Each mock records its calls (so tests can assert which RPCs ran) and
//! optionally simulates failure via an `AtomicBool` flag.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use futures::Stream;
use parking_lot::Mutex;

use anima_proxy::{
    Account, AnimaCall, AnimaProxyError, AnimaProxyResult, Profile, SessionDescriptor,
};
use arcan_proxy::{ArcanCall, ArcanProxyError, ArcanProxyResult};
use haima_proxy::{HaimaCall, HaimaProxyError, HaimaProxyResult, LedgerEntry, WalletBalance};
use lago_proxy::{LagoCall, LagoProxyError, LagoProxyResult};
use life_runtime_proto::life::v1 as pb;

/// Mock arcan substrate.
#[derive(Default, Clone)]
pub struct MockArcan {
    pub create_agent_calls: Arc<Mutex<Vec<String>>>,
    pub destroy_agent_calls: Arc<Mutex<Vec<String>>>,
    /// When set, the next `create_agent` returns an error (then resets).
    pub fail_next: Arc<AtomicBool>,
}

impl MockArcan {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn inject_fault(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl ArcanCall for MockArcan {
    async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
        self.create_agent_calls.lock().push(sid.to_string());
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(ArcanProxyError::Substrate(tonic::Status::internal(
                "injected fault",
            )));
        }
        Ok(format!("agent-{sid}"))
    }
    async fn destroy_agent(&self, sid: &str) -> ArcanProxyResult<()> {
        self.destroy_agent_calls.lock().push(sid.to_string());
        Ok(())
    }
    async fn dispatch_message(
        &self,
        _sid: &str,
        _content: &str,
    ) -> ArcanProxyResult<Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, tonic::Status>> + Send>>>
    {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<pb::AgentEvent, tonic::Status>>(8);
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(pb::AgentEvent {
                    record: None,
                    kind: pb::AgentEventKind::Token as i32,
                }))
                .await;
            let _ = tx
                .send(Ok(pb::AgentEvent {
                    record: None,
                    kind: pb::AgentEventKind::Finish as i32,
                }))
                .await;
        });
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

/// Mock lago substrate.
#[derive(Default, Clone)]
pub struct MockLago {
    pub open_namespace_calls: Arc<Mutex<Vec<String>>>,
    pub close_namespace_calls: Arc<Mutex<Vec<String>>>,
    /// (namespace, event_type) pairs recorded by `append_event`. Used by
    /// admin-plane integration tests to assert that saga state landed in
    /// `system/lifed/saga/<saga_id>`.
    pub append_event_calls: Arc<Mutex<Vec<(String, String)>>>,
    pub fail_next: Arc<AtomicBool>,
}

impl MockLago {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn inject_fault(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl LagoCall for MockLago {
    async fn open_namespace(&self, sid: &str) -> LagoProxyResult<String> {
        self.open_namespace_calls.lock().push(sid.to_string());
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(LagoProxyError::Substrate(tonic::Status::internal(
                "injected fault",
            )));
        }
        Ok(format!("session/{sid}"))
    }
    async fn close_namespace(&self, ns: &str) -> LagoProxyResult<()> {
        self.close_namespace_calls.lock().push(ns.to_string());
        Ok(())
    }
    async fn read(
        &self,
        _sid: &str,
        _from: u64,
        _limit: u32,
    ) -> LagoProxyResult<Pin<Box<dyn Stream<Item = Result<pb::EventRecord, tonic::Status>> + Send>>>
    {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<pb::EventRecord, tonic::Status>>(1);
        drop(tx);
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
    async fn subscribe(
        &self,
        sid: &str,
        from: u64,
    ) -> LagoProxyResult<Pin<Box<dyn Stream<Item = Result<pb::EventRecord, tonic::Status>> + Send>>>
    {
        self.read(sid, from, 0).await
    }
    async fn get_blob(&self, _ns: &str, _sha256: &str) -> LagoProxyResult<(Vec<u8>, String)> {
        Ok((b"empty".to_vec(), "application/octet-stream".to_string()))
    }
    async fn idem_lookup(&self, _key: &[u8]) -> LagoProxyResult<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn idem_persist(&self, _key: &[u8], _response: Vec<u8>) -> LagoProxyResult<()> {
        Ok(())
    }
    async fn append_event(
        &self,
        namespace: &str,
        event_type: &str,
        _payload: Vec<u8>,
    ) -> LagoProxyResult<()> {
        self.append_event_calls
            .lock()
            .push((namespace.to_string(), event_type.to_string()));
        Ok(())
    }
}

/// Mock haima substrate.
#[derive(Default, Clone)]
pub struct MockHaima {
    pub bind_wallet_calls: Arc<Mutex<Vec<(String, String)>>>,
    pub unbind_wallet_calls: Arc<Mutex<Vec<String>>>,
    pub debit_calls: Arc<Mutex<Vec<(String, String, u64)>>>,
    pub balances: Arc<Mutex<HashMap<String, u64>>>,
    pub fail_next: Arc<AtomicBool>,
}

impl MockHaima {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn inject_fault(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl HaimaCall for MockHaima {
    async fn bind_wallet(&self, sid: &str, project_id: &str) -> HaimaProxyResult<String> {
        self.bind_wallet_calls
            .lock()
            .push((sid.to_string(), project_id.to_string()));
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(HaimaProxyError::Substrate(tonic::Status::internal(
                "injected fault",
            )));
        }
        let wallet_id = format!("wallet-{sid}");
        self.balances.lock().insert(wallet_id.clone(), 1_000_000);
        Ok(wallet_id)
    }
    async fn unbind_wallet(&self, wallet_id: &str) -> HaimaProxyResult<()> {
        self.unbind_wallet_calls.lock().push(wallet_id.to_string());
        Ok(())
    }
    async fn get_balance(
        &self,
        _user_id: &str,
        _project_id: &str,
    ) -> HaimaProxyResult<WalletBalance> {
        Ok(WalletBalance {
            micros: 1_000_000,
            currency: "USDC".to_string(),
        })
    }
    async fn statement(
        &self,
        _user_id: &str,
        _project_id: &str,
        _since_ms: i64,
        _until_ms: i64,
        _limit: u32,
    ) -> HaimaProxyResult<Pin<Box<dyn Stream<Item = Result<LedgerEntry, tonic::Status>> + Send>>>
    {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LedgerEntry, tonic::Status>>(1);
        drop(tx);
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
    async fn debit(
        &self,
        user_id: &str,
        project_id: &str,
        amount_micros: u64,
        _sid: &str,
        _reason: &str,
    ) -> HaimaProxyResult<(String, WalletBalance)> {
        self.debit_calls
            .lock()
            .push((user_id.to_string(), project_id.to_string(), amount_micros));
        Ok((
            "entry-1".to_string(),
            WalletBalance {
                micros: 1_000_000_u64.saturating_sub(amount_micros),
                currency: "USDC".to_string(),
            },
        ))
    }
    async fn transfer(
        &self,
        _from_user: &str,
        _from_project: &str,
        _to_user: &str,
        _to_project: &str,
        _amount_micros: u64,
        _memo: &str,
    ) -> HaimaProxyResult<(String, WalletBalance, WalletBalance)> {
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

/// Mock anima substrate.
#[derive(Default, Clone)]
pub struct MockAnima {
    pub register_session_calls: Arc<Mutex<Vec<(String, String)>>>,
    pub mark_closed_calls: Arc<Mutex<Vec<String>>>,
    pub revoke_calls: Arc<Mutex<Vec<String>>>,
    pub fail_next: Arc<AtomicBool>,
}

impl MockAnima {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn inject_fault(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl AnimaCall for MockAnima {
    async fn register_session(&self, sid: &str, user_id: &str) -> AnimaProxyResult<()> {
        self.register_session_calls
            .lock()
            .push((sid.to_string(), user_id.to_string()));
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(AnimaProxyError::Substrate(tonic::Status::internal(
                "injected fault",
            )));
        }
        Ok(())
    }
    async fn mark_session_closed(&self, sid: &str) -> AnimaProxyResult<()> {
        self.mark_closed_calls.lock().push(sid.to_string());
        Ok(())
    }
    async fn get_account(&self, user_id: &str) -> AnimaProxyResult<Account> {
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
    async fn update_profile(&self, user_id: &str, profile: Profile) -> AnimaProxyResult<Account> {
        let mut a = self.get_account(user_id).await?;
        a.profile = profile;
        Ok(a)
    }
    async fn list_sessions(
        &self,
        _user_id: &str,
        _include_closed: bool,
        _limit: u32,
    ) -> AnimaProxyResult<Vec<SessionDescriptor>> {
        Ok(vec![])
    }
    async fn revoke_session(&self, sid: &str) -> AnimaProxyResult<()> {
        self.revoke_calls.lock().push(sid.to_string());
        Ok(())
    }
}

/// Bundle of all four mocks — sub-phase B test fixtures + dev daemon path.
#[derive(Clone, Default)]
pub struct MockSubstrates {
    pub arcan: MockArcan,
    pub lago: MockLago,
    pub haima: MockHaima,
    pub anima: MockAnima,
}

impl MockSubstrates {
    pub fn new() -> Self {
        Self::default()
    }
}
