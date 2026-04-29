//! life.v1.Identity — public-plane identity namespace.
//!
//! Handlers ≤20 LOC each. `RevokeSession` updates the in-memory blocklist
//! and evicts the routing entry; the snapshot publisher (run on a 30 s
//! tick by bootstrap) writes `revoked_sids.json` for substrates to poll.
//!
//! ## Pool bracketing — Sub-phase E
//!
//! Sub-phase E pushes pool bracketing inside each proxy crate's
//! `Pooled<C>` adapter (Spec C₂ §7). Identity handlers no longer need a
//! `pools` field — every `self.anima.<rpc>()` call brackets internally.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use aios_proto::aios::v1 as aios_v1;
use anima_proxy::AnimaCall;
use life_runtime_proto::life::v1 as pb;

use crate::auth::blocklist::RevokedSidSet;
use crate::auth::capability::CapabilityClaims;
use crate::routing::cache::RoutingCache;

pub struct IdentityService {
    pub anima: Arc<dyn AnimaCall>,
    pub routing: Arc<RoutingCache>,
    pub revoked: Arc<RevokedSidSet>,
}

impl IdentityService {
    pub fn new(
        anima: Arc<dyn AnimaCall>,
        routing: Arc<RoutingCache>,
        revoked: Arc<RevokedSidSet>,
    ) -> Self {
        Self {
            anima,
            routing,
            revoked,
        }
    }

    fn claims<T>(req: &Request<T>) -> Result<&CapabilityClaims, Status> {
        req.extensions()
            .get::<CapabilityClaims>()
            .ok_or_else(|| Status::unauthenticated("missing capability claims"))
    }
}

#[tonic::async_trait]
impl pb::identity_server::Identity for IdentityService {
    async fn me(&self, req: Request<pb::IdentityEmpty>) -> Result<Response<pb::Account>, Status> {
        let claims = Self::claims(&req)?;
        let acct = self
            .anima
            .get_account(&claims.user_id)
            .await
            .map_err(Status::from)?;
        Ok(Response::new(account_to_pb(acct)))
    }

    async fn update_profile(
        &self,
        req: Request<pb::UpdateProfileReq>,
    ) -> Result<Response<pb::Account>, Status> {
        let claims = Self::claims(&req)?.clone();
        let body = req.into_inner();
        let prof = body.profile.unwrap_or_default();
        let acct = self
            .anima
            .update_profile(&claims.user_id, profile_from_pb(prof))
            .await
            .map_err(Status::from)?;
        Ok(Response::new(account_to_pb(acct)))
    }

    async fn list_sessions(
        &self,
        req: Request<pb::ListSessionsReq>,
    ) -> Result<Response<pb::SessionList>, Status> {
        let claims = Self::claims(&req)?;
        let descs = self
            .anima
            .list_sessions(
                &claims.user_id,
                req.get_ref().include_closed,
                req.get_ref().limit,
            )
            .await
            .map_err(Status::from)?;
        Ok(Response::new(pb::SessionList {
            sessions: descs
                .into_iter()
                .map(|d| pb::SessionDescriptor {
                    sid: Some(aios_v1::SessionId { value: d.sid }),
                    project_id: d.project_id,
                    opened_at: Some(prost_types::Timestamp {
                        seconds: d.opened_at_ms / 1000,
                        nanos: 0,
                    }),
                    closed_at: Some(prost_types::Timestamp {
                        seconds: d.closed_at_ms / 1000,
                        nanos: 0,
                    }),
                    label: d.label,
                })
                .collect(),
        }))
    }

    async fn revoke_session(
        &self,
        req: Request<pb::IdentitySessionRef>,
    ) -> Result<Response<pb::IdentityEmpty>, Status> {
        let _claims = Self::claims(&req)?;
        let sid = req
            .into_inner()
            .sid
            .ok_or_else(|| Status::invalid_argument("sid"))?;
        self.anima
            .revoke_session(&sid.value)
            .await
            .map_err(Status::from)?;
        self.revoked.insert(&sid);
        self.routing.evict(&sid);
        Ok(Response::new(pb::IdentityEmpty {}))
    }
}

fn account_to_pb(a: anima_proxy::Account) -> pb::Account {
    pb::Account {
        user_id: a.user_id,
        handle: a.handle,
        display_name: a.display_name,
        email: a.email,
        tier: a.tier,
        created_at: Some(prost_types::Timestamp {
            seconds: a.created_at_ms / 1000,
            nanos: 0,
        }),
        profile: Some(pb::Profile {
            bio: a.profile.bio,
            avatar_blob_ref: a.profile.avatar_blob_ref,
            preferences: a.profile.preferences,
        }),
    }
}

fn profile_from_pb(p: pb::Profile) -> anima_proxy::Profile {
    anima_proxy::Profile {
        bio: p.bio,
        avatar_blob_ref: p.avatar_blob_ref,
        preferences: p.preferences,
    }
}
