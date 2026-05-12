//! Tonic service implementation for `anima.v1.IdentitySubstrate`.
//!
//! BRO-1019 — Phase 4 of the Topology B substrate-stub gap close.
//! Pulls peer creds from request extensions (placed there by
//! [`crate::admin::listener::AdminConn`]), runs the policy check via
//! the same [`crate::admin::AdminPolicy`] that gates `CustodyOracle`,
//! and delegates to the in-process [`super::IdentityState`].
//!
//! Wire types come from `anima_substrate_proto::anima::v1`.
//!
//! Auth: SO_PEERCRED + group membership (`life-runtime`); NO bearer
//! tokens. Same model as the sibling [`crate::admin::CustodyOracleService`]
//! (Spec D D-Sub-E). lifed runs as a member of the `life-runtime` group
//! and dials soma's admin UDS to reach both services.

use std::sync::Arc;
use std::time::SystemTime;

use anima_substrate_proto::anima::v1::{
    self as anima_pb, identity_substrate_server::IdentitySubstrate,
};
use tonic::{Request, Response, Status};

use crate::admin::listener::AdminConnInfo;
use crate::admin::peercred::PeerCred;
use crate::admin::policy::{AdminOp, AdminPolicy};
use crate::identity::state::{IdentityState, ProfileRecord, SessionRecord};

/// `anima.v1.IdentitySubstrate` service backed by a shared
/// `Arc<IdentityState>` + the soma admin policy. The state is shared
/// across every per-connection service clone so substrate-plane writes
/// are immediately visible to subsequent reads (test invariant covered
/// by `tests/topology_b_e2e_anima.rs`).
#[derive(Clone)]
pub struct IdentitySubstrateService {
    state: Arc<IdentityState>,
    policy: Arc<AdminPolicy>,
}

impl IdentitySubstrateService {
    pub fn new(state: Arc<IdentityState>, policy: Arc<AdminPolicy>) -> Self {
        Self { state, policy }
    }

    /// Expose the underlying state. Integration tests + the daemon
    /// bootstrap share the `Arc<IdentityState>` so they can observe
    /// side effects of substrate-plane writes.
    pub fn state(&self) -> &Arc<IdentityState> {
        &self.state
    }

    /// Extract the peer creds from a request's extensions. Returns
    /// [`Status::internal`] on absence — the soma admin acceptor is
    /// expected to always wrap connections in `AdminConn`.
    fn cred<T>(req: &Request<T>) -> Result<PeerCred, Status> {
        req.extensions()
            .get::<AdminConnInfo>()
            .map(|c| c.cred)
            .ok_or_else(|| Status::internal("admin connection lacks PeerCred"))
    }

    /// Mirror [`crate::admin::service::CustodyOracleService::validate_user_id`]
    /// so the identity substrate enforces the same character whitelist.
    fn validate_user_id(user_id: &str) -> Result<(), Status> {
        if user_id.is_empty() || user_id.len() > 64 {
            return Err(Status::invalid_argument(format!(
                "user_id length out of range (1..=64): {}",
                user_id.len()
            )));
        }
        for c in user_id.chars() {
            let ok = c.is_ascii_alphanumeric() || c == '_' || c == '-';
            if !ok {
                return Err(Status::invalid_argument(format!(
                    "user_id contains disallowed character {c:?}; \
                     must match [a-zA-Z0-9_-]+ (no /, \\, .., :, whitespace, etc.)"
                )));
            }
        }
        Ok(())
    }

    fn validate_sid(sid: &str) -> Result<(), Status> {
        if sid.is_empty() {
            return Err(Status::invalid_argument("empty sid"));
        }
        if sid.len() > 256 {
            return Err(Status::invalid_argument(format!(
                "sid length out of range (1..=256): {}",
                sid.len()
            )));
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl IdentitySubstrate for IdentitySubstrateService {
    async fn register_session(
        &self,
        req: Request<anima_pb::RegisterSessionReq>,
    ) -> Result<Response<anima_pb::RegisterSessionResp>, Status> {
        let cred = Self::cred(&req)?;
        // Identity-data is governed by the SAME closed-by-default
        // policy as CustodyOracle. We reuse `AdminOp::SignAuth` as the
        // policy hook for every identity RPC — the policy check is
        // gid-driven and the per-op enum is informational only (see
        // `AdminPolicy::check` body). A future ticket can split out a
        // dedicated `IdentityOp::*` variant set if per-RPC granularity
        // is ever required; today every member of the `life-runtime`
        // group is admitted to every identity RPC.
        self.policy.check(&cred, AdminOp::SignAuth)?;
        let inner = req.into_inner();
        let sid_proto = inner
            .sid
            .ok_or_else(|| Status::invalid_argument("missing sid"))?;
        Self::validate_sid(&sid_proto.value)?;
        Self::validate_user_id(&inner.user_id)?;
        // Phase-4 substrate doesn't take a project_id on register
        // (the proxy's `register_session` signature only carries
        // (sid, user_id) — project_id binds at saga step 2 via
        // arcand.CreateAgent's label field). Leave empty for now.
        let opened_at = self
            .state
            .register_session(&sid_proto.value, &inner.user_id, "");
        Ok(Response::new(anima_pb::RegisterSessionResp {
            opened_at: Some(prost_types::Timestamp::from(SystemTime::from(opened_at))),
        }))
    }

    async fn mark_session_closed(
        &self,
        req: Request<anima_pb::MarkSessionClosedReq>,
    ) -> Result<Response<anima_pb::MarkSessionClosedResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SignAuth)?;
        let inner = req.into_inner();
        let sid_proto = inner
            .sid
            .ok_or_else(|| Status::invalid_argument("missing sid"))?;
        Self::validate_sid(&sid_proto.value)?;
        // Idempotent: unknown sid still returns Ok.
        self.state.mark_session_closed(&sid_proto.value);
        Ok(Response::new(anima_pb::MarkSessionClosedResp {}))
    }

    async fn get_account(
        &self,
        req: Request<anima_pb::GetAccountReq>,
    ) -> Result<Response<anima_pb::Account>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SignAuth)?;
        let inner = req.into_inner();
        Self::validate_user_id(&inner.user_id)?;
        let acc = self.state.get_or_create_account(&inner.user_id);
        Ok(Response::new(account_to_proto(acc)))
    }

    async fn update_profile(
        &self,
        req: Request<anima_pb::UpdateProfileReq>,
    ) -> Result<Response<anima_pb::Account>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SignAuth)?;
        let inner = req.into_inner();
        Self::validate_user_id(&inner.user_id)?;
        let profile = inner
            .profile
            .ok_or_else(|| Status::invalid_argument("missing profile"))?;
        let acc = self
            .state
            .update_profile(&inner.user_id, profile_from_proto(profile));
        Ok(Response::new(account_to_proto(acc)))
    }

    async fn list_sessions(
        &self,
        req: Request<anima_pb::ListSessionsReq>,
    ) -> Result<Response<anima_pb::ListSessionsResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SignAuth)?;
        let inner = req.into_inner();
        Self::validate_user_id(&inner.user_id)?;
        let sessions = self
            .state
            .list_sessions(&inner.user_id, inner.include_closed, inner.limit);
        Ok(Response::new(anima_pb::ListSessionsResp {
            sessions: sessions.into_iter().map(session_to_proto).collect(),
        }))
    }

    async fn revoke_session(
        &self,
        req: Request<anima_pb::RevokeSessionReq>,
    ) -> Result<Response<anima_pb::RevokeSessionResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SignAuth)?;
        let inner = req.into_inner();
        let sid_proto = inner
            .sid
            .ok_or_else(|| Status::invalid_argument("missing sid"))?;
        Self::validate_sid(&sid_proto.value)?;
        // Idempotent: unknown sid still returns Ok.
        self.state.revoke_session(&sid_proto.value);
        Ok(Response::new(anima_pb::RevokeSessionResp {}))
    }
}

// ── Proto <-> internal conversions ─────────────────────────────────────────────

fn account_to_proto(acc: super::state::AccountRecord) -> anima_pb::Account {
    anima_pb::Account {
        user_id: acc.user_id,
        handle: acc.handle,
        display_name: acc.display_name,
        email: acc.email,
        tier: acc.tier,
        created_at_ms: acc.created_at.timestamp_millis(),
        profile: Some(profile_to_proto(acc.profile)),
    }
}

fn profile_to_proto(p: ProfileRecord) -> anima_pb::Profile {
    anima_pb::Profile {
        bio: p.bio,
        avatar_blob_ref: p.avatar_blob_ref,
        preferences: p.preferences,
    }
}

fn profile_from_proto(p: anima_pb::Profile) -> ProfileRecord {
    ProfileRecord {
        bio: p.bio,
        avatar_blob_ref: p.avatar_blob_ref,
        preferences: p.preferences,
    }
}

fn session_to_proto(s: SessionRecord) -> anima_pb::SessionDescriptor {
    anima_pb::SessionDescriptor {
        sid: s.sid,
        project_id: s.project_id,
        opened_at_ms: s.opened_at.timestamp_millis(),
        // 0 = still open. Matches the proxy's struct default.
        closed_at_ms: s.closed_at.map(|t| t.timestamp_millis()).unwrap_or(0),
        label: s.label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::policy::AdminPolicy;
    use anima_substrate_proto::aios_v1::v1 as aios_v1;

    fn fixture_service() -> IdentitySubstrateService {
        IdentitySubstrateService::new(
            Arc::new(IdentityState::new()),
            Arc::new(AdminPolicy::permissive()),
        )
    }

    fn make_request<T>(value: T, cred: PeerCred) -> Request<T> {
        let mut req = Request::new(value);
        req.extensions_mut().insert(AdminConnInfo { cred });
        req
    }

    fn dev_cred() -> PeerCred {
        PeerCred {
            pid: 0,
            uid: 1000,
            gid: 1000,
        }
    }

    #[tokio::test]
    async fn register_session_returns_opened_at_and_persists() {
        let svc = fixture_service();
        let req = make_request(
            anima_pb::RegisterSessionReq {
                sid: Some(aios_v1::SessionId {
                    value: "sid-1".into(),
                }),
                user_id: "alice".into(),
            },
            dev_cred(),
        );
        let resp = svc.register_session(req).await.unwrap().into_inner();
        assert!(resp.opened_at.is_some());
        assert_eq!(svc.state.session_count(), 1);
    }

    #[tokio::test]
    async fn register_session_is_idempotent_on_sid() {
        let svc = fixture_service();
        for _ in 0..3 {
            let req = make_request(
                anima_pb::RegisterSessionReq {
                    sid: Some(aios_v1::SessionId {
                        value: "sid-id".into(),
                    }),
                    user_id: "alice".into(),
                },
                dev_cred(),
            );
            svc.register_session(req).await.unwrap();
        }
        assert_eq!(svc.state.session_count(), 1);
    }

    #[tokio::test]
    async fn get_account_materializes_default_record() {
        let svc = fixture_service();
        let req = make_request(
            anima_pb::GetAccountReq {
                user_id: "alice".into(),
            },
            dev_cred(),
        );
        let resp = svc.get_account(req).await.unwrap().into_inner();
        assert_eq!(resp.user_id, "alice");
        assert_eq!(resp.handle, "@alice");
        assert_eq!(resp.tier, "free");
        assert!(resp.profile.is_some());
    }

    #[tokio::test]
    async fn update_profile_persists_and_returns_updated() {
        let svc = fixture_service();
        let new_profile = anima_pb::Profile {
            bio: "hello".into(),
            avatar_blob_ref: vec![9, 9, 9],
            preferences: Default::default(),
        };
        let req = make_request(
            anima_pb::UpdateProfileReq {
                user_id: "bob".into(),
                profile: Some(new_profile),
            },
            dev_cred(),
        );
        let resp = svc.update_profile(req).await.unwrap().into_inner();
        let p = resp.profile.expect("profile present");
        assert_eq!(p.bio, "hello");
        assert_eq!(p.avatar_blob_ref, vec![9, 9, 9]);

        // Second get_account observes the updated profile.
        let probe_req = make_request(
            anima_pb::GetAccountReq {
                user_id: "bob".into(),
            },
            dev_cred(),
        );
        let probe = svc.get_account(probe_req).await.unwrap().into_inner();
        let pp = probe.profile.expect("profile present");
        assert_eq!(pp.bio, "hello");
    }

    #[tokio::test]
    async fn list_sessions_filters_closed_and_user() {
        let svc = fixture_service();
        for (sid, user) in [("a", "alice"), ("b", "alice"), ("c", "bob")] {
            let req = make_request(
                anima_pb::RegisterSessionReq {
                    sid: Some(aios_v1::SessionId { value: sid.into() }),
                    user_id: user.into(),
                },
                dev_cred(),
            );
            svc.register_session(req).await.unwrap();
        }

        // Close "a".
        let close_req = make_request(
            anima_pb::MarkSessionClosedReq {
                sid: Some(aios_v1::SessionId { value: "a".into() }),
            },
            dev_cred(),
        );
        svc.mark_session_closed(close_req).await.unwrap();

        // alice with include_closed = false should see only b.
        let req = make_request(
            anima_pb::ListSessionsReq {
                user_id: "alice".into(),
                include_closed: false,
                limit: 0,
            },
            dev_cred(),
        );
        let resp = svc.list_sessions(req).await.unwrap().into_inner();
        assert_eq!(resp.sessions.len(), 1);
        assert_eq!(resp.sessions[0].sid, "b");

        // alice with include_closed = true should see both.
        let req2 = make_request(
            anima_pb::ListSessionsReq {
                user_id: "alice".into(),
                include_closed: true,
                limit: 0,
            },
            dev_cred(),
        );
        let resp2 = svc.list_sessions(req2).await.unwrap().into_inner();
        assert_eq!(resp2.sessions.len(), 2);
    }

    #[tokio::test]
    async fn revoke_session_is_idempotent() {
        let svc = fixture_service();
        let reg = make_request(
            anima_pb::RegisterSessionReq {
                sid: Some(aios_v1::SessionId {
                    value: "to-revoke".into(),
                }),
                user_id: "alice".into(),
            },
            dev_cred(),
        );
        svc.register_session(reg).await.unwrap();

        for _ in 0..2 {
            let req = make_request(
                anima_pb::RevokeSessionReq {
                    sid: Some(aios_v1::SessionId {
                        value: "to-revoke".into(),
                    }),
                },
                dev_cred(),
            );
            svc.revoke_session(req).await.unwrap();
        }
        // Unknown sid still returns Ok.
        let req = make_request(
            anima_pb::RevokeSessionReq {
                sid: Some(aios_v1::SessionId {
                    value: "never-existed".into(),
                }),
            },
            dev_cred(),
        );
        svc.revoke_session(req).await.unwrap();
    }

    #[tokio::test]
    async fn user_id_validation_rejects_path_traversal() {
        let svc = fixture_service();
        let req = make_request(
            anima_pb::GetAccountReq {
                user_id: "alice/../admin".into(),
            },
            dev_cred(),
        );
        let err = svc.get_account(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn missing_peer_cred_returns_internal() {
        let svc = fixture_service();
        // Manually build a request without AdminConnInfo extension.
        let req = Request::new(anima_pb::GetAccountReq {
            user_id: "alice".into(),
        });
        let err = svc.get_account(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    #[tokio::test]
    async fn strict_policy_rejects_stranger() {
        let svc = IdentitySubstrateService::new(
            Arc::new(IdentityState::new()),
            Arc::new(AdminPolicy::strict(1500)),
        );
        let req = make_request(
            anima_pb::GetAccountReq {
                user_id: "alice".into(),
            },
            PeerCred {
                pid: 0,
                uid: 1,
                gid: 1,
            },
        );
        let err = svc.get_account(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }
}
