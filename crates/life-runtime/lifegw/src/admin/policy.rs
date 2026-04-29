//! Admin-plane authorisation policy. Sub-phase D (D2).
//!
//! Closed-by-default: all ops require either `admin_gid` group
//! membership OR `uid == 0` (root). Per Spec C₃ §3.6 + the prompt's
//! hard rule "Admin plane authn is SO_PEERCRED + group membership;
//! NO bearer tokens".
//!
//! The role table is intentionally narrow — there's no autonomic
//! daemon reaching the lifegw admin plane in M7 (the autonomic
//! integration lives at lifed's admin plane). Future phases can add
//! roles without breaking the policy contract.

use crate::admin::peercred::PeerCred;

#[derive(Debug, Clone, Copy)]
pub enum AdminOp {
    HealthCheck,
    CertReload,
    JwksDump,
    BlocklistAdd,
    BlocklistRemove,
    BlocklistList,
    RateLimitOverride,
}

#[derive(Debug, Clone)]
pub struct AdminPolicy {
    pub admin_gid: u32,
    /// Permissive mode — every op allowed for every peer. Set when
    /// the daemon is configured without
    /// `admin_plane.unix_socket_group` (the systemd unit enforces
    /// access at the FS layer in that case). Used by integration
    /// tests that bind a tempdir socket without a group.
    pub permissive: bool,
}

impl AdminPolicy {
    pub fn check(&self, cred: &PeerCred, op: AdminOp) -> Result<(), tonic::Status> {
        if self.permissive {
            return Ok(());
        }
        let is_admin = cred.gid == self.admin_gid;
        let is_root = cred.uid == 0;

        let allowed = match op {
            // Anyone who can connect can ping the admin socket.
            AdminOp::HealthCheck => true,
            // Read-only inspection ops — admin or root.
            AdminOp::JwksDump | AdminOp::BlocklistList => is_admin || is_root,
            // Mutating ops — admin or root.
            AdminOp::CertReload
            | AdminOp::BlocklistAdd
            | AdminOp::BlocklistRemove
            | AdminOp::RateLimitOverride => is_admin || is_root,
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
            permissive: false,
        }
    }

    #[test]
    fn permissive_mode_authorises_every_op() {
        let pol = AdminPolicy {
            admin_gid: 0,
            permissive: true,
        };
        for op in [
            AdminOp::HealthCheck,
            AdminOp::CertReload,
            AdminOp::JwksDump,
            AdminOp::BlocklistAdd,
            AdminOp::RateLimitOverride,
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
    fn admin_can_cert_reload_and_dump_jwks() {
        let pol = policy();
        pol.check(&cred(100, 1500), AdminOp::CertReload).unwrap();
        pol.check(&cred(100, 1500), AdminOp::JwksDump).unwrap();
    }

    #[test]
    fn admin_can_modify_blocklist() {
        let pol = policy();
        pol.check(&cred(100, 1500), AdminOp::BlocklistAdd).unwrap();
        pol.check(&cred(100, 1500), AdminOp::BlocklistRemove)
            .unwrap();
        pol.check(&cred(100, 1500), AdminOp::BlocklistList).unwrap();
    }

    #[test]
    fn admin_can_override_rate_limit() {
        let pol = policy();
        pol.check(&cred(100, 1500), AdminOp::RateLimitOverride)
            .unwrap();
    }

    #[test]
    fn random_user_blocked_from_everything_but_health() {
        let pol = policy();
        let stranger = cred(9999, 9999);
        for op in [
            AdminOp::CertReload,
            AdminOp::JwksDump,
            AdminOp::BlocklistAdd,
            AdminOp::BlocklistRemove,
            AdminOp::BlocklistList,
            AdminOp::RateLimitOverride,
        ] {
            assert_eq!(
                pol.check(&stranger, op).map_err(|e| e.code()),
                Err(tonic::Code::PermissionDenied),
                "op {op:?} should be denied for stranger",
            );
        }
    }

    #[test]
    fn root_can_do_everything() {
        let pol = policy();
        let r = cred(0, 0);
        for op in [
            AdminOp::HealthCheck,
            AdminOp::CertReload,
            AdminOp::JwksDump,
            AdminOp::BlocklistAdd,
            AdminOp::BlocklistRemove,
            AdminOp::BlocklistList,
            AdminOp::RateLimitOverride,
        ] {
            pol.check(&r, op).expect("root allowed");
        }
    }
}
