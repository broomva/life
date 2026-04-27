//! Admin-plane policy table — closed by default per Spec C₂ §5.3.
//!
//! Authorisation is decided per `(PeerCred, AdminOp)`. There are three
//! roles, gated by the peer's primary GID/UID:
//!
//! - `is_admin` — peer's primary GID matches `admin_gid` (typically the
//!   `life-admin` group). Read-only ops + force-close.
//! - `is_autonomic` — peer's UID matches `autonomic_uid`. Sub-phase C₆
//!   wires the autonomic daemon's regulation surface; for now the field
//!   is `Option` and only set when the autonomic UID is configured.
//! - `is_root` — peer's UID is 0. Bypass for emergency / dangerous ops.
//!
//! Sub-phase C MVS only checks the peer's primary GID. Spec C₆ adds full
//! supplementary-group inspection.

use crate::auth::peercred::PeerCred;

#[derive(Debug, Clone, Copy)]
pub enum AdminOp {
    HealthCheck,
    SessionsListAll,
    SessionsForceClose,
    SessionsSuspend,
    IdempotencyLookup,
    SagaListInflight,
    SagaShow,
    SagaForceCompensate,
    RoutingCacheDump,
    RoutingCacheEvict,
    RoutingCacheRebuildFromLago,
}

#[derive(Debug, Clone)]
pub struct AdminPolicy {
    pub admin_gid: u32,
    pub autonomic_uid: Option<u32>,
    /// Permissive mode — every op allowed for every peer. Set when the
    /// daemon is configured without `admin_plane.unix_socket_group` (the
    /// systemd unit is expected to enforce socket access at the
    /// filesystem layer in that case). Used by integration tests that
    /// bind a tempdir socket without a group, and by single-user dev
    /// boxes where the operator IS the runtime user.
    pub permissive: bool,
}

impl AdminPolicy {
    pub fn check(&self, cred: &PeerCred, op: AdminOp) -> Result<(), tonic::Status> {
        if self.permissive {
            return Ok(());
        }

        let is_admin = cred.gid == self.admin_gid;
        let is_autonomic = self.autonomic_uid.map(|u| u == cred.uid).unwrap_or(false);
        let is_root = cred.uid == 0;

        let allowed = match op {
            // Anyone who can connect can ping the admin socket.
            AdminOp::HealthCheck => true,
            AdminOp::SessionsListAll => is_admin || is_autonomic || is_root,
            AdminOp::SessionsForceClose => is_admin || is_root,
            AdminOp::SessionsSuspend => is_autonomic || is_root,
            AdminOp::IdempotencyLookup => is_admin || is_root,
            AdminOp::SagaListInflight => is_admin || is_autonomic || is_root,
            AdminOp::SagaShow => is_admin || is_autonomic || is_root,
            AdminOp::SagaForceCompensate => is_root, // dangerous
            AdminOp::RoutingCacheDump => is_admin || is_autonomic || is_root,
            AdminOp::RoutingCacheEvict => is_admin || is_root,
            AdminOp::RoutingCacheRebuildFromLago => is_root,
        };

        if !allowed {
            return Err(tonic::Status::permission_denied(format!(
                "uid={} gid={} not authorised for {:?}",
                cred.uid, cred.gid, op,
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(uid: u32, gid: u32) -> PeerCred {
        PeerCred { pid: 0, uid, gid }
    }

    fn policy() -> AdminPolicy {
        AdminPolicy {
            admin_gid: 1500,
            autonomic_uid: Some(2000),
            permissive: false,
        }
    }

    #[test]
    fn permissive_mode_authorises_every_op() {
        let pol = AdminPolicy {
            admin_gid: 0,
            autonomic_uid: None,
            permissive: true,
        };
        for op in [
            AdminOp::HealthCheck,
            AdminOp::SessionsForceClose,
            AdminOp::SagaForceCompensate,
            AdminOp::RoutingCacheRebuildFromLago,
        ] {
            pol.check(&cred(9999, 9999), op)
                .expect("permissive allows all");
        }
    }

    #[test]
    fn health_check_allows_anyone() {
        let pol = policy();
        pol.check(&cred(9999, 9999), AdminOp::HealthCheck).unwrap();
    }

    #[test]
    fn admin_can_list_and_force_close() {
        let pol = policy();
        pol.check(&cred(100, 1500), AdminOp::SessionsListAll)
            .unwrap();
        pol.check(&cred(100, 1500), AdminOp::SessionsForceClose)
            .unwrap();
    }

    #[test]
    fn admin_cannot_suspend() {
        let pol = policy();
        let err = pol
            .check(&cred(100, 1500), AdminOp::SessionsSuspend)
            .expect_err("admin not allowed to suspend");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn autonomic_can_suspend_and_list() {
        let pol = policy();
        pol.check(&cred(2000, 99), AdminOp::SessionsSuspend)
            .unwrap();
        pol.check(&cred(2000, 99), AdminOp::SessionsListAll)
            .unwrap();
    }

    #[test]
    fn root_can_force_compensate() {
        let pol = policy();
        pol.check(&cred(0, 0), AdminOp::SagaForceCompensate)
            .unwrap();
    }

    #[test]
    fn nonadmin_cannot_force_compensate() {
        let pol = policy();
        let err = pol
            .check(&cred(100, 1500), AdminOp::SagaForceCompensate)
            .expect_err("admin not allowed to force-compensate");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn random_user_blocked_from_everything_but_health() {
        let pol = policy();
        let stranger = cred(9999, 9999);
        for op in [
            AdminOp::SessionsListAll,
            AdminOp::SessionsForceClose,
            AdminOp::SessionsSuspend,
            AdminOp::IdempotencyLookup,
            AdminOp::SagaListInflight,
            AdminOp::SagaShow,
            AdminOp::SagaForceCompensate,
            AdminOp::RoutingCacheDump,
            AdminOp::RoutingCacheEvict,
            AdminOp::RoutingCacheRebuildFromLago,
        ] {
            assert_eq!(
                pol.check(&stranger, op).map_err(|e| e.code()),
                Err(tonic::Code::PermissionDenied),
                "op {op:?} should be denied for stranger",
            );
        }
    }
}
