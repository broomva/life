//! Development mocks for arcan/lago/haima/anima substrates.
//!
//! Sub-phase A uses these as the default substrate adapters for both the
//! daemon entrypoint and integration tests, so that lifed can be exercised
//! end-to-end before the real `*-proxy` crates are wired in sub-phase B.
//!
//! Sub-phase B retires these modules (the real proxy crates take over the
//! daemon path) but the test harness keeps them as deterministic fixtures.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

/// Mock arcan substrate. Records calls; returns canned responses.
#[derive(Default, Clone)]
pub struct MockArcan {
    pub create_agent_calls: Arc<Mutex<Vec<String>>>,
    pub destroy_agent_calls: Arc<Mutex<Vec<String>>>,
}

impl MockArcan {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pretend to create an agent; returns a canned `agent_id`.
    pub async fn create_agent(&self, sid: &str) -> Result<String, String> {
        self.create_agent_calls.lock().push(sid.to_string());
        Ok(format!("agent-{sid}"))
    }

    pub async fn destroy_agent(&self, sid: &str) -> Result<(), String> {
        self.destroy_agent_calls.lock().push(sid.to_string());
        Ok(())
    }
}

/// Mock lago substrate.
#[derive(Default, Clone)]
pub struct MockLago {
    pub open_namespace_calls: Arc<Mutex<Vec<String>>>,
    pub close_namespace_calls: Arc<Mutex<Vec<String>>>,
}

impl MockLago {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn open_namespace(&self, sid: &str) -> Result<String, String> {
        self.open_namespace_calls.lock().push(sid.to_string());
        Ok(format!("session/{sid}"))
    }

    pub async fn close_namespace(&self, ns: &str) -> Result<(), String> {
        self.close_namespace_calls.lock().push(ns.to_string());
        Ok(())
    }
}

/// Mock haima substrate.
#[derive(Default, Clone)]
pub struct MockHaima {
    pub bind_wallet_calls: Arc<Mutex<Vec<(String, String)>>>,
    pub unbind_wallet_calls: Arc<Mutex<Vec<String>>>,
    pub balances: Arc<Mutex<HashMap<String, u64>>>,
}

impl MockHaima {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn bind_wallet(&self, sid: &str, project_id: &str) -> Result<String, String> {
        self.bind_wallet_calls
            .lock()
            .push((sid.to_string(), project_id.to_string()));
        let wallet_id = format!("wallet-{sid}");
        self.balances.lock().insert(wallet_id.clone(), 1_000_000);
        Ok(wallet_id)
    }

    pub async fn unbind_wallet(&self, wallet_id: &str) -> Result<(), String> {
        self.unbind_wallet_calls.lock().push(wallet_id.to_string());
        Ok(())
    }
}

/// Mock anima substrate.
#[derive(Default, Clone)]
pub struct MockAnima {
    pub register_session_calls: Arc<Mutex<Vec<(String, String)>>>,
    pub mark_closed_calls: Arc<Mutex<Vec<String>>>,
}

impl MockAnima {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register_session(&self, sid: &str, user_id: &str) -> Result<(), String> {
        self.register_session_calls
            .lock()
            .push((sid.to_string(), user_id.to_string()));
        Ok(())
    }

    pub async fn mark_session_closed(&self, sid: &str) -> Result<(), String> {
        self.mark_closed_calls.lock().push(sid.to_string());
        Ok(())
    }
}

/// Bundle of all four mocks — sub-phase A test fixtures + daemon entrypoint
/// adapter source.
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
