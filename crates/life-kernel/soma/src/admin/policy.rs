//! Authorisation policy for the soma admin custody-oracle UDS.
//!
//! Spec D D-Sub-E: closed-by-default. All custody-oracle ops require
//! either `admin_gid` group membership (primary OR supplementary) OR
//! `uid == 0` (root). Mirrors lifegw's `admin/policy.rs` shape; the
//! per-RPC operation enum is custody-shaped rather than gateway-shaped.
//!
//! NO bearer tokens — admin-plane authn is SO_PEERCRED + group
//! membership exclusively (Spec D §"Phasing > D-Sub-E" rule).

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::admin::peercred::{PeerCred, supplementary_gids_of_uid};

/// Custody-oracle operation tag — one variant per RPC method on the
/// `life.admin.kernel.v1.CustodyOracle` service.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum AdminOp {
    /// `kernel.SignAuth` — sign a digest with the user's auth (P-256) key.
    SignAuth,
    /// `kernel.SignWallet` — sign a digest with the user's wallet (secp256k1) key.
    SignWallet,
    /// `kernel.GetAuthPubkey` — bootstrap-only pubkey fetch.
    GetAuthPubkey,
    /// `kernel.GetWalletPubkey` — bootstrap-only pubkey fetch.
    GetWalletPubkey,
}

/// SO_PEERCRED-driven authorisation policy.
#[derive(Debug, Clone)]
pub struct AdminPolicy {
    pub admin_gid: u32,
    /// Permissive mode admits every peer for every op. Used by
    /// integration tests that bind a tempdir socket without a configured
    /// group.
    pub permissive: bool,
    /// Per-uid supplementary-group cache. Same fail-closed semantics as
    /// lifegw — error responses are negative-cached so a uid spamming
    /// the admin socket can't drive a syscall storm.
    #[doc(hidden)]
    supp_groups: Arc<RwLock<HashMap<u32, Vec<u32>>>>,
}

impl AdminPolicy {
    /// Construct a permissive (test) policy.
    pub fn permissive() -> Self {
        Self {
            admin_gid: 0,
            permissive: true,
            supp_groups: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Construct a strict policy with the given admin GID.
    pub fn strict(admin_gid: u32) -> Self {
        Self {
            admin_gid,
            permissive: false,
            supp_groups: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Authorise `cred` for `op`. Returns `Ok(())` on success and a
    /// `tonic::Status::permission_denied` on failure.
    pub fn check(&self, cred: &PeerCred, op: AdminOp) -> Result<(), tonic::Status> {
        if self.permissive {
            return Ok(());
        }
        let is_root = cred.uid == 0;
        if is_root {
            // Root is permitted for everything — same convention as
            // lifegw's admin policy.
            let _ = op;
            return Ok(());
        }

        let primary_match = cred.gid == self.admin_gid;
        let admin = if primary_match {
            true
        } else {
            // Slow path — supplementary group lookup. Fails CLOSED on
            // syscall error.
            self.supplementary_membership(cred.uid).map_err(|e| {
                tracing::warn!(
                    uid = cred.uid,
                    error = %e,
                    "soma admin group lookup failed — fail-closed deny"
                );
                tonic::Status::permission_denied(format!(
                    "uid={} group lookup failed: {e}",
                    cred.uid,
                ))
            })?
        };

        if !admin {
            return Err(tonic::Status::permission_denied(format!(
                "uid={} gid={} not authorised for {:?}",
                cred.uid, cred.gid, op,
            )));
        }
        Ok(())
    }

    /// True if `uid`'s supplementary group list contains `admin_gid`.
    /// Caches both success and failure paths (negative cache prevents
    /// syscall floods from non-existent uids).
    fn supplementary_membership(&self, uid: u32) -> std::io::Result<bool> {
        if let Some(groups) = self.supp_groups.read().get(&uid).cloned() {
            return Ok(groups.contains(&self.admin_gid));
        }
        match supplementary_gids_of_uid(uid) {
            Ok(groups) => {
                let in_group = groups.contains(&self.admin_gid);
                self.supp_groups.write().insert(uid, groups);
                Ok(in_group)
            }
            Err(e) => {
                self.supp_groups.write().entry(uid).or_default();
                Err(e)
            }
        }
    }
}

impl Default for AdminPolicy {
    fn default() -> Self {
        Self::permissive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(uid: u32, gid: u32) -> PeerCred {
        PeerCred { pid: 0, uid, gid }
    }

    #[test]
    fn permissive_admits_every_op() {
        let pol = AdminPolicy::permissive();
        for op in [
            AdminOp::SignAuth,
            AdminOp::SignWallet,
            AdminOp::GetAuthPubkey,
            AdminOp::GetWalletPubkey,
        ] {
            pol.check(&cred(9999, 9999), op)
                .expect("permissive admits all");
        }
    }

    #[test]
    fn root_admits_every_op() {
        let pol = AdminPolicy::strict(1500);
        for op in [
            AdminOp::SignAuth,
            AdminOp::SignWallet,
            AdminOp::GetAuthPubkey,
            AdminOp::GetWalletPubkey,
        ] {
            pol.check(&cred(0, 0), op).expect("root admits all");
        }
    }

    #[test]
    fn primary_gid_match_admits() {
        let pol = AdminPolicy::strict(1500);
        pol.check(&cred(100, 1500), AdminOp::SignAuth).unwrap();
        pol.check(&cred(100, 1500), AdminOp::SignWallet).unwrap();
    }

    #[test]
    fn stranger_denied() {
        let pol = AdminPolicy::strict(1500);
        let err = pol.check(&cred(1, 1), AdminOp::SignAuth).unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn missing_user_fails_closed() {
        let pol = AdminPolicy::strict(1500);
        // uid u32::MAX cannot exist in /etc/passwd. Linux: getgrouplist
        // errors → deny. macOS: fallback returns empty → not in group → deny.
        let outcome = pol.check(&cred(u32::MAX, 0), AdminOp::SignAuth);
        assert!(outcome.is_err());
        assert_eq!(outcome.unwrap_err().code(), tonic::Code::PermissionDenied);
    }
}
