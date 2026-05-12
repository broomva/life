//! Identity substrate state — in-memory account + session registry
//! consumed by [`super::IdentitySubstrateService`] under Topology B.
//!
//! Phase 4 (BRO-1019) scope:
//!
//! - One [`AccountRecord`] per `user_id`. Accounts materialise lazily
//!   on first access with default starting values (handle `@{user_id}`,
//!   tier `"free"`, empty profile). Matches the proxy's pre-BRO-1019
//!   stub shape so the wire change is invisible to lifed's tests.
//! - One [`SessionRecord`] per `sid`. Registered via `RegisterSession`,
//!   updated via `MarkSessionClosed` / `RevokeSession`. Indexed by
//!   `user_id` for `ListSessions`.
//! - All operations are idempotent on their primary key (`user_id`
//!   or `sid`) so saga compensation paths stay clean (Spec C₂ §4.2).
//!
//! What this is NOT (yet):
//!
//! - Persistent. A future ticket will wire `anima-lago` so every
//!   mutating call also produces an `EventKind::Custom("anima.*", ...)`
//!   lago event. Today the in-memory `IdentityState` is the only source
//!   of truth for the wire. Mirrors haima's Phase F2 / BRO-1018
//!   `HaimaState` deferral.
//!
//! Concurrency: `IdentityState` is internally locked (`std::sync::RwLock`
//! over a tokio-friendly granularity — we never hold the lock across
//! `await`). All public methods are synchronous and intentionally
//! cheap (no I/O). The substrate handlers wrap them inside async fns
//! so the trait shape stays uniform with the proxy.

use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default tier for a freshly-materialised account.
pub const DEFAULT_TIER: &str = "free";

/// Errors that can arise inside [`IdentityState`].
///
/// Phase 4 has no error-producing failure modes (all ops materialise
/// missing records or are idempotent), but the enum is non-exhaustive
/// so future tickets can add `AccountNotFound` / `SessionNotFound`
/// without a breaking change.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IdentityStateError {
    #[error("session not found: {0}")]
    SessionNotFound(String),
}

pub type IdentityStateResult<T> = Result<T, IdentityStateError>;

/// An account record — the canonical user-scoped identity shape
/// projected by the substrate to lifed. Mirrors the
/// `anima_proxy::client::Account` struct verbatim so the wire mapping
/// is a trivial copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub user_id: String,
    pub handle: String,
    pub display_name: String,
    pub email: String,
    pub tier: String,
    pub created_at: DateTime<Utc>,
    pub profile: ProfileRecord,
}

/// Profile sub-record. Mirrors `anima_proxy::client::Profile`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileRecord {
    pub bio: String,
    pub avatar_blob_ref: Vec<u8>,
    pub preferences: HashMap<String, String>,
}

/// A session descriptor — what `ListSessions` returns and what
/// `register_session` materializes substrate-side. Mirrors
/// `anima_proxy::client::SessionDescriptor`. Sessions store their
/// `opened_at` + (optional) `closed_at` as full `DateTime<Utc>` for
/// internal use; the wire shape exposes them as Unix-ms (i64).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub sid: String,
    pub user_id: String,
    pub project_id: String,
    pub opened_at: DateTime<Utc>,
    /// `None` when the session is still open. Set by
    /// `mark_closed` / `revoke`.
    pub closed_at: Option<DateTime<Utc>>,
    /// When non-`None`, the session was explicitly revoked (vs. closed
    /// gracefully). For Phase 4 we don't differentiate revocation from
    /// closure in the wire shape — both flip `closed_at_ms` — but the
    /// substrate keeps the distinction in case a future ticket exposes
    /// it.
    pub revoked_at: Option<DateTime<Utc>>,
    pub label: String,
}

#[derive(Debug, Default)]
struct StateInner {
    /// Accounts indexed by `user_id`.
    accounts: HashMap<String, AccountRecord>,
    /// Sessions indexed by `sid` (the wire-level primary key).
    sessions: HashMap<String, SessionRecord>,
}

/// Process-wide identity substrate state. Cheap to clone (internally
/// `Arc<RwLock<…>>`). Constructed once in soma bootstrap and shared by
/// every `IdentitySubstrateService` instance.
#[derive(Debug, Default)]
pub struct IdentityState {
    inner: RwLock<StateInner>,
}

impl IdentityState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Idempotent session register. Returns the `opened_at` timestamp
    /// of the canonical (first-observed) registration so a saga retry
    /// observes the same value the first call returned.
    ///
    /// If `(sid, user_id)` is already known, the existing `opened_at`
    /// is returned and no state mutates. If the same `sid` is observed
    /// for a different `user_id` the substrate keeps the first binding
    /// and ignores the conflicting one — lifed treats sid as a per-user
    /// primary key, and changing it post-bind would imply a bug.
    pub fn register_session(&self, sid: &str, user_id: &str, project_id: &str) -> DateTime<Utc> {
        // Fast path: already registered.
        {
            let guard = self.inner.read().expect("poisoned");
            if let Some(existing) = guard.sessions.get(sid) {
                return existing.opened_at;
            }
        }

        let now = Utc::now();
        let record = SessionRecord {
            sid: sid.to_string(),
            user_id: user_id.to_string(),
            project_id: project_id.to_string(),
            opened_at: now,
            closed_at: None,
            revoked_at: None,
            label: String::new(),
        };

        let mut guard = self.inner.write().expect("poisoned");
        // Re-check under the write lock — another caller may have
        // raced ahead. Idempotency is the contract.
        guard
            .sessions
            .entry(sid.to_string())
            .or_insert(record)
            .opened_at
    }

    /// Idempotent: mark the session as gracefully closed. Sets
    /// `closed_at` to now if not already set. Sessions that don't
    /// exist return `Ok(())` so saga compensation stays clean.
    pub fn mark_session_closed(&self, sid: &str) {
        let mut guard = self.inner.write().expect("poisoned");
        if let Some(s) = guard.sessions.get_mut(sid)
            && s.closed_at.is_none()
        {
            s.closed_at = Some(Utc::now());
        }
    }

    /// Idempotent: revoke a session. Sets BOTH `closed_at` and
    /// `revoked_at` if not already set. Sessions that don't exist
    /// return `Ok(())` so saga compensation stays clean.
    pub fn revoke_session(&self, sid: &str) {
        let mut guard = self.inner.write().expect("poisoned");
        if let Some(s) = guard.sessions.get_mut(sid) {
            let now = Utc::now();
            if s.closed_at.is_none() {
                s.closed_at = Some(now);
            }
            if s.revoked_at.is_none() {
                s.revoked_at = Some(now);
            }
        }
    }

    /// Look up an account by `user_id`, materialising a default record
    /// on first access. Matches the pre-BRO-1019 anima-proxy stub
    /// shape so lifed's Identity-handler tests keep passing.
    pub fn get_or_create_account(&self, user_id: &str) -> AccountRecord {
        // Fast read path.
        {
            let guard = self.inner.read().expect("poisoned");
            if let Some(existing) = guard.accounts.get(user_id) {
                return existing.clone();
            }
        }

        let now = Utc::now();
        let record = AccountRecord {
            user_id: user_id.to_string(),
            handle: format!("@{user_id}"),
            display_name: user_id.to_string(),
            email: format!("{user_id}@example.com"),
            tier: DEFAULT_TIER.to_string(),
            created_at: now,
            profile: ProfileRecord::default(),
        };

        let mut guard = self.inner.write().expect("poisoned");
        // Re-check under the write lock; another caller may have
        // raced ahead.
        guard
            .accounts
            .entry(user_id.to_string())
            .or_insert(record)
            .clone()
    }

    /// Replace the profile sub-record. Materialises the account on
    /// first access (same default-account shape as `get_or_create_account`).
    /// Returns the updated record.
    pub fn update_profile(&self, user_id: &str, profile: ProfileRecord) -> AccountRecord {
        // Ensure the account exists first.
        let _ = self.get_or_create_account(user_id);

        let mut guard = self.inner.write().expect("poisoned");
        let acc = guard
            .accounts
            .get_mut(user_id)
            .expect("account materialised above");
        acc.profile = profile;
        acc.clone()
    }

    /// Enumerate sessions for a user. Sessions are ordered by
    /// `opened_at` ascending. When `include_closed` is false, sessions
    /// whose `closed_at` is set are filtered out. `limit == 0` means
    /// "no cap".
    pub fn list_sessions(
        &self,
        user_id: &str,
        include_closed: bool,
        limit: u32,
    ) -> Vec<SessionRecord> {
        let guard = self.inner.read().expect("poisoned");
        let cap = if limit == 0 {
            usize::MAX
        } else {
            limit as usize
        };
        let mut out: Vec<SessionRecord> = guard
            .sessions
            .values()
            .filter(|s| s.user_id == user_id)
            .filter(|s| include_closed || s.closed_at.is_none())
            .cloned()
            .collect();
        out.sort_by_key(|s| s.opened_at);
        out.truncate(cap);
        out
    }

    /// Test-only probe.
    #[doc(hidden)]
    pub fn account_count(&self) -> usize {
        self.inner.read().expect("poisoned").accounts.len()
    }

    /// Test-only probe.
    #[doc(hidden)]
    pub fn session_count(&self) -> usize {
        self.inner.read().expect("poisoned").sessions.len()
    }

    /// Test-only probe.
    #[doc(hidden)]
    pub fn session(&self, sid: &str) -> Option<SessionRecord> {
        self.inner
            .read()
            .expect("poisoned")
            .sessions
            .get(sid)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_session_is_idempotent() {
        let st = IdentityState::new();
        let t1 = st.register_session("sid-1", "alice", "proj-A");
        let t2 = st.register_session("sid-1", "alice", "proj-A");
        assert_eq!(t1, t2, "opened_at is sticky across re-registers");
        assert_eq!(st.session_count(), 1);
    }

    #[test]
    fn register_session_ignores_user_change() {
        let st = IdentityState::new();
        let _ = st.register_session("sid-2", "alice", "proj-A");
        // Re-registering with a different user_id is a no-op — the
        // first binding wins. Defensive against lifed bugs.
        let _ = st.register_session("sid-2", "bob", "proj-A");
        let s = st.session("sid-2").expect("registered");
        assert_eq!(s.user_id, "alice");
    }

    #[test]
    fn mark_session_closed_sets_timestamp_idempotently() {
        let st = IdentityState::new();
        let _ = st.register_session("sid-c", "alice", "proj-A");
        st.mark_session_closed("sid-c");
        let s1 = st.session("sid-c").expect("present");
        let first = s1.closed_at.expect("closed_at set");

        // Re-close: timestamp must NOT be overwritten.
        st.mark_session_closed("sid-c");
        let s2 = st.session("sid-c").expect("present");
        assert_eq!(s2.closed_at, Some(first));

        // Unknown sid: idempotent no-op.
        st.mark_session_closed("sid-never-existed");
    }

    #[test]
    fn revoke_session_sets_both_timestamps_idempotently() {
        let st = IdentityState::new();
        let _ = st.register_session("sid-r", "alice", "proj-A");
        st.revoke_session("sid-r");
        let s = st.session("sid-r").expect("present");
        let first_closed = s.closed_at.expect("closed_at set");
        let first_revoked = s.revoked_at.expect("revoked_at set");

        // Re-revoke: both timestamps stay frozen.
        st.revoke_session("sid-r");
        let s2 = st.session("sid-r").expect("present");
        assert_eq!(s2.closed_at, Some(first_closed));
        assert_eq!(s2.revoked_at, Some(first_revoked));
    }

    #[test]
    fn get_or_create_account_returns_defaults() {
        let st = IdentityState::new();
        let a1 = st.get_or_create_account("alice");
        assert_eq!(a1.user_id, "alice");
        assert_eq!(a1.handle, "@alice");
        assert_eq!(a1.tier, DEFAULT_TIER);
        assert!(a1.email.contains("alice"));

        // Idempotency — repeated calls return the same record.
        let a2 = st.get_or_create_account("alice");
        assert_eq!(a1.user_id, a2.user_id);
        assert_eq!(st.account_count(), 1);
    }

    #[test]
    fn update_profile_mutates_account_in_place() {
        let st = IdentityState::new();
        let mut prefs = HashMap::new();
        prefs.insert("theme".to_string(), "dark".to_string());
        let new_profile = ProfileRecord {
            bio: "test bio".into(),
            avatar_blob_ref: vec![1, 2, 3],
            preferences: prefs,
        };
        let updated = st.update_profile("bob", new_profile.clone());
        assert_eq!(updated.user_id, "bob");
        assert_eq!(updated.profile.bio, "test bio");
        assert_eq!(updated.profile.avatar_blob_ref, vec![1, 2, 3]);
        assert_eq!(
            updated.profile.preferences.get("theme"),
            Some(&"dark".to_string())
        );

        // A subsequent get_or_create returns the updated profile.
        let probe = st.get_or_create_account("bob");
        assert_eq!(probe.profile.bio, "test bio");
    }

    #[test]
    fn list_sessions_orders_and_filters() {
        let st = IdentityState::new();
        let _ = st.register_session("sid-a", "alice", "proj-A");
        // Force a different opened_at for sid-b by waiting a beat.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = st.register_session("sid-b", "alice", "proj-B");
        let _ = st.register_session("sid-c", "bob", "proj-X"); // different user

        // bob's sessions should not appear in alice's list.
        let alice = st.list_sessions("alice", false, 0);
        assert_eq!(alice.len(), 2);
        assert_eq!(alice[0].sid, "sid-a");
        assert_eq!(alice[1].sid, "sid-b");

        // Close sid-a; without include_closed it disappears.
        st.mark_session_closed("sid-a");
        let alice_open = st.list_sessions("alice", false, 0);
        assert_eq!(alice_open.len(), 1);
        assert_eq!(alice_open[0].sid, "sid-b");

        // include_closed brings it back.
        let alice_all = st.list_sessions("alice", true, 0);
        assert_eq!(alice_all.len(), 2);

        // limit caps the result.
        let alice_one = st.list_sessions("alice", true, 1);
        assert_eq!(alice_one.len(), 1);
    }
}
