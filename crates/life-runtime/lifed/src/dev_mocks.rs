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
use haima_proxy::{
    HaimaCall, HaimaProxyError, HaimaProxyResult, LedgerEntry, WalletBalance, X402PayOutcome,
};
use lago_proxy::{LagoCall, LagoProxyError, LagoProxyResult};
use life_runtime_proto::life::v1 as pb;

/// Mock arcan substrate.
#[derive(Default, Clone)]
pub struct MockArcan {
    pub create_agent_calls: Arc<Mutex<Vec<String>>>,
    pub destroy_agent_calls: Arc<Mutex<Vec<String>>>,
    pub dispatch_calls: Arc<Mutex<Vec<(String, String)>>>,
    /// BRO-1206: captures the per-call `model` override passed to
    /// `dispatch_message`. Tests assert this to prove the override
    /// travels end-to-end from `Agent.CreateSession` (stored on the
    /// routing-cache entry) through `Agent.SendMessage` to the
    /// substrate-call boundary.
    #[allow(clippy::type_complexity)]
    pub dispatch_models: Arc<Mutex<Vec<(String, Option<String>)>>>,
    /// Captures the client tool definitions passed to
    /// `dispatch_message`. Tests assert this to prove the chat
    /// surface's tools travel from `Agent.SendMessage`
    /// (`tool_definitions` bytes) through lifed's fanout pump to the
    /// substrate-call boundary.
    #[allow(clippy::type_complexity)]
    pub dispatch_tools: Arc<Mutex<Vec<(String, Vec<serde_json::Value>)>>>,
    /// When set, the next `create_agent` returns an error (then resets).
    pub fail_next: Arc<AtomicBool>,
    /// Sub-phase D: sustained failure mode for chaos tests.
    pub force_fail: Arc<AtomicBool>,
    /// Sub-phase D8: backpressure-test pump sender. Lives in the Mock
    /// so tests can call `flush_token` repeatedly. Use [`Self::install_pump`]
    /// to wire it up before issuing the dispatch.
    pub event_pump_tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<()>>>>,
    /// The receiving half of the pump. Taken by `dispatch_message` once
    /// per dispatch — when present, that dispatch waits on signals from
    /// `flush_token`; when absent, the canned (Token, Finish) sequence
    /// runs immediately as in sub-phase A/B/C.
    pub event_pump_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<()>>>>,
}

impl MockArcan {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn inject_fault(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
    pub fn set_force_fail(&self, enabled: bool) {
        self.force_fail.store(enabled, Ordering::SeqCst);
    }
    fn force_fail_status(&self) -> Option<tonic::Status> {
        if self.force_fail.load(Ordering::SeqCst) {
            Some(tonic::Status::unavailable("arcan down (chaos test)"))
        } else {
            None
        }
    }
}

impl MockArcan {
    /// Sub-phase D8: install a pump channel before issuing a
    /// `dispatch_message`. The next dispatch will listen on the
    /// receiver and emit one Token event per `flush_token` call.
    /// When the test wants the dispatch to terminate, it calls
    /// [`Self::close_pump`] to drop the sender; the dispatch then
    /// emits Finish and the stream closes.
    pub fn install_pump(&self) {
        let (tx, rx) = tokio::sync::mpsc::channel::<()>(1024);
        *self.event_pump_tx.lock() = Some(tx);
        *self.event_pump_rx.lock() = Some(rx);
    }

    /// Drop the pump sender — the next attempt to flush is a no-op,
    /// and any in-flight dispatch sees `recv() -> None` and emits Finish.
    pub fn close_pump(&self) {
        *self.event_pump_tx.lock() = None;
    }

    /// Push one Token event into the active dispatch. No-op if no
    /// pump is installed or if the bounded buffer (1024) is saturated.
    pub async fn flush_token(&self) {
        if let Some(tx) = self.event_pump_tx.lock().clone() {
            let _ = tx.try_send(());
        }
    }
}

#[async_trait]
impl ArcanCall for MockArcan {
    async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
        self.create_agent_calls.lock().push(sid.to_string());
        if let Some(s) = self.force_fail_status() {
            return Err(ArcanProxyError::Substrate(s));
        }
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
        sid: &str,
        content: &str,
        model: Option<&str>,
        tools: &[serde_json::Value],
    ) -> ArcanProxyResult<Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, tonic::Status>> + Send>>>
    {
        // BRO-1206: mock records the dispatch (sid, content) — model
        // override is captured separately on `dispatch_models`, and
        // client tool definitions on `dispatch_tools`, so tests can
        // assert plumbing without changing existing `dispatch_calls`
        // semantics.
        self.dispatch_calls
            .lock()
            .push((sid.to_string(), content.to_string()));
        self.dispatch_models
            .lock()
            .push((sid.to_string(), model.map(str::to_string)));
        self.dispatch_tools
            .lock()
            .push((sid.to_string(), tools.to_vec()));
        if let Some(s) = self.force_fail_status() {
            return Err(ArcanProxyError::Substrate(s));
        }
        // Tests opt into the pump pattern by calling
        // `MockArcan::install_pump()` BEFORE issuing the dispatch. When
        // the slot holds a sender, this dispatch listens for `flush_token`
        // calls and emits one extra Token per signal. When no pump is
        // installed (the default for sub-phase A/B/C tests), the
        // canned (Token, Finish) sequence runs immediately and the
        // stream closes — same behaviour as before D.
        let pump_rx = self.event_pump_rx.lock().take();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<pb::AgentEvent, tonic::Status>>(8);
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(pb::AgentEvent {
                    record: None,
                    kind: pb::AgentEventKind::Token as i32,
                }))
                .await;
            if let Some(mut rx) = pump_rx {
                // Drain pump signals as long as someone is willing to
                // queue them. When all senders are dropped, recv()
                // returns None and we emit Finish.
                while rx.recv().await.is_some() {
                    if tx
                        .send(Ok(pb::AgentEvent {
                            record: None,
                            kind: pb::AgentEventKind::Token as i32,
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
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
    /// Sub-phase D2: prefix queries against the mock backend. Tests can
    /// pre-populate `seeded_namespaces` for cold-start replay scenarios.
    pub list_namespaces_calls: Arc<Mutex<Vec<String>>>,
    pub seeded_namespaces: Arc<Mutex<Vec<String>>>,
    pub fail_next: Arc<AtomicBool>,
    /// Sub-phase D7: when set, every RPC call returns Unavailable. Used
    /// by chaos tests to simulate a sustained lago outage.
    pub force_fail: Arc<AtomicBool>,
}

impl MockLago {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn inject_fault(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
    /// Set sustained failure mode (chaos test). Every RPC returns
    /// `Unavailable` until [`Self::clear_force_fail`] is called.
    pub fn set_force_fail(&self, enabled: bool) {
        self.force_fail.store(enabled, Ordering::SeqCst);
    }
    /// Seed `list_namespaces` results for cold-start replay tests.
    pub fn seed_namespaces(&self, namespaces: Vec<String>) {
        *self.seeded_namespaces.lock() = namespaces;
    }
    fn force_fail_status(&self) -> Option<tonic::Status> {
        if self.force_fail.load(Ordering::SeqCst) {
            Some(tonic::Status::unavailable("lago down (chaos test)"))
        } else {
            None
        }
    }
}

#[async_trait]
impl LagoCall for MockLago {
    async fn open_namespace(&self, sid: &str) -> LagoProxyResult<String> {
        self.open_namespace_calls.lock().push(sid.to_string());
        if let Some(s) = self.force_fail_status() {
            return Err(LagoProxyError::Substrate(s));
        }
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(LagoProxyError::Substrate(tonic::Status::internal(
                "injected fault",
            )));
        }
        Ok(format!("session/{sid}"))
    }
    async fn close_namespace(&self, ns: &str) -> LagoProxyResult<()> {
        self.close_namespace_calls.lock().push(ns.to_string());
        if let Some(s) = self.force_fail_status() {
            return Err(LagoProxyError::Substrate(s));
        }
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
        if let Some(s) = self.force_fail_status() {
            return Err(LagoProxyError::Substrate(s));
        }
        Ok(None)
    }
    async fn idem_persist(&self, _key: &[u8], _response: Vec<u8>) -> LagoProxyResult<()> {
        if let Some(s) = self.force_fail_status() {
            return Err(LagoProxyError::Substrate(s));
        }
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
        if let Some(s) = self.force_fail_status() {
            return Err(LagoProxyError::Substrate(s));
        }
        Ok(())
    }
    async fn list_namespaces(&self, prefix: &str) -> LagoProxyResult<Vec<String>> {
        self.list_namespaces_calls.lock().push(prefix.to_string());
        if let Some(s) = self.force_fail_status() {
            return Err(LagoProxyError::Substrate(s));
        }
        let seeded = self.seeded_namespaces.lock().clone();
        Ok(seeded
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .collect())
    }
}

/// Mock haima substrate.
#[derive(Default, Clone)]
#[allow(clippy::type_complexity)]
pub struct MockHaima {
    pub bind_wallet_calls: Arc<Mutex<Vec<(String, String)>>>,
    pub unbind_wallet_calls: Arc<Mutex<Vec<String>>>,
    pub debit_calls: Arc<Mutex<Vec<(String, String, u64)>>>,
    pub transfer_calls: Arc<Mutex<Vec<(String, String, String, String, u64)>>>,
    pub x402_pay_calls: Arc<Mutex<Vec<(String, String, String)>>>,
    pub balances: Arc<Mutex<HashMap<String, u64>>>,
    pub fail_next: Arc<AtomicBool>,
    pub force_fail: Arc<AtomicBool>,
}

impl MockHaima {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn inject_fault(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
    pub fn set_force_fail(&self, enabled: bool) {
        self.force_fail.store(enabled, Ordering::SeqCst);
    }
    fn force_fail_status(&self) -> Option<tonic::Status> {
        if self.force_fail.load(Ordering::SeqCst) {
            Some(tonic::Status::unavailable("haima down (chaos test)"))
        } else {
            None
        }
    }
}

#[async_trait]
impl HaimaCall for MockHaima {
    async fn bind_wallet(&self, sid: &str, project_id: &str) -> HaimaProxyResult<String> {
        self.bind_wallet_calls
            .lock()
            .push((sid.to_string(), project_id.to_string()));
        if let Some(s) = self.force_fail_status() {
            return Err(HaimaProxyError::Substrate(s));
        }
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
        if let Some(s) = self.force_fail_status() {
            return Err(HaimaProxyError::Substrate(s));
        }
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
        if let Some(s) = self.force_fail_status() {
            return Err(HaimaProxyError::Substrate(s));
        }
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
        from_user: &str,
        from_project: &str,
        to_user: &str,
        to_project: &str,
        amount_micros: u64,
        _memo: &str,
    ) -> HaimaProxyResult<(String, WalletBalance, WalletBalance)> {
        self.transfer_calls.lock().push((
            from_user.to_string(),
            from_project.to_string(),
            to_user.to_string(),
            to_project.to_string(),
            amount_micros,
        ));
        if let Some(s) = self.force_fail_status() {
            return Err(HaimaProxyError::Substrate(s));
        }
        Ok((
            format!(
                "entry-{}-{}",
                self.transfer_calls.lock().len(),
                amount_micros
            ),
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
    async fn x402_pay(
        &self,
        user_id: &str,
        project_id: &str,
        resource_url: &str,
        _network: &str,
        _max_amount_micros: Option<i64>,
    ) -> HaimaProxyResult<X402PayOutcome> {
        self.x402_pay_calls.lock().push((
            user_id.to_string(),
            project_id.to_string(),
            resource_url.to_string(),
        ));
        if let Some(s) = self.force_fail_status() {
            return Err(HaimaProxyError::Substrate(s));
        }
        // Canned "settled" outcome on base-sepolia for handler tests.
        Ok(X402PayOutcome {
            status: "settled".to_string(),
            tx_hash: "0xmocktx".to_string(),
            network: "eip155:84532".to_string(),
            recipient: "0x036CbD53842c5426634e7929541eC2318f3dCF7e".to_string(),
            micro_credits: 50,
            declined_reason: String::new(),
            settled: true,
            resource_body: b"{\"ok\":true}".to_vec(),
            resource_status: 200,
        })
    }
}

/// Mock anima substrate.
#[derive(Default, Clone)]
pub struct MockAnima {
    pub register_session_calls: Arc<Mutex<Vec<(String, String)>>>,
    pub mark_closed_calls: Arc<Mutex<Vec<String>>>,
    pub revoke_calls: Arc<Mutex<Vec<String>>>,
    pub fail_next: Arc<AtomicBool>,
    pub force_fail: Arc<AtomicBool>,
}

impl MockAnima {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn inject_fault(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
    pub fn set_force_fail(&self, enabled: bool) {
        self.force_fail.store(enabled, Ordering::SeqCst);
    }
    fn force_fail_status(&self) -> Option<tonic::Status> {
        if self.force_fail.load(Ordering::SeqCst) {
            Some(tonic::Status::unavailable("anima down (chaos test)"))
        } else {
            None
        }
    }
}

#[async_trait]
impl AnimaCall for MockAnima {
    async fn register_session(&self, sid: &str, user_id: &str) -> AnimaProxyResult<()> {
        self.register_session_calls
            .lock()
            .push((sid.to_string(), user_id.to_string()));
        if let Some(s) = self.force_fail_status() {
            return Err(AnimaProxyError::Substrate(s));
        }
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
