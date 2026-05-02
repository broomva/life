//! Tonic service implementation for the soma admin custody-oracle
//! (Spec D D-Sub-E).
//!
//! Mirrors the lifegw `GatewayAdminService` shape: every RPC pulls
//! peer creds from request extensions (placed there by
//! [`crate::admin::listener::AdminConn`]), runs the policy check, and
//! delegates to the in-process [`crate::admin::keys::InProcessCustodyKeys`]
//! store.
//!
//! Wire types come from `life_kernel_proto::custody`.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::admin::listener::AdminConnInfo;
use crate::admin::peercred::PeerCred;
use crate::admin::policy::{AdminOp, AdminPolicy};

use life_kernel_proto::custody as oracle_pb;

/// Re-export of the in-process key store used by the service. Operators
/// who swap in a TPM / HSM implementation provide a different value
/// here and rely on the same trait surface (just a `&self` set of
/// per-user signing operations).
pub use crate::admin::keys::InProcessCustodyKeys;

/// Trait the service depends on. Implementing this yourself lets
/// production deploys swap the in-process backend for TPM / HSM /
/// remote signing without touching the soma admin handler. The default
/// shipped implementation is [`InProcessCustodyKeys`].
pub trait CustodyKeyStore: Send + Sync + 'static {
    fn sign_auth_digest(&self, user_id: &str, digest: &[u8; 32]) -> Result<[u8; 64], Status>;
    fn sign_wallet_digest(&self, user_id: &str, digest: &[u8; 32]) -> Result<[u8; 65], Status>;
    fn auth_pubkey_sec1(&self, user_id: &str) -> Result<[u8; 33], Status>;
    fn wallet_pubkey_sec1_uncompressed(&self, user_id: &str) -> Result<[u8; 65], Status>;
}

impl CustodyKeyStore for InProcessCustodyKeys {
    fn sign_auth_digest(&self, user_id: &str, digest: &[u8; 32]) -> Result<[u8; 64], Status> {
        InProcessCustodyKeys::sign_auth_digest(self, user_id, digest).map_err(anima_err_to_status)
    }
    fn sign_wallet_digest(&self, user_id: &str, digest: &[u8; 32]) -> Result<[u8; 65], Status> {
        InProcessCustodyKeys::sign_wallet_digest(self, user_id, digest).map_err(anima_err_to_status)
    }
    fn auth_pubkey_sec1(&self, user_id: &str) -> Result<[u8; 33], Status> {
        InProcessCustodyKeys::auth_pubkey_sec1(self, user_id).map_err(anima_err_to_status)
    }
    fn wallet_pubkey_sec1_uncompressed(&self, user_id: &str) -> Result<[u8; 65], Status> {
        InProcessCustodyKeys::wallet_pubkey_sec1_uncompressed(self, user_id)
            .map_err(anima_err_to_status)
    }
}

fn anima_err_to_status(err: anima_core::error::AnimaError) -> Status {
    use anima_core::error::AnimaError;
    match err {
        AnimaError::Crypto(msg) if msg.contains("not provisioned") => Status::not_found(msg),
        other => Status::internal(other.to_string()),
    }
}

/// `life.admin.kernel.v1.CustodyOracle` service backed by a swappable
/// key store + policy.
#[derive(Clone)]
pub struct CustodyOracleService<S: CustodyKeyStore> {
    policy: Arc<AdminPolicy>,
    store: Arc<S>,
}

impl<S: CustodyKeyStore> CustodyOracleService<S> {
    pub fn new(policy: Arc<AdminPolicy>, store: Arc<S>) -> Self {
        Self { policy, store }
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

    fn validate_user_id(user_id: &str) -> Result<(), Status> {
        // Mirror VaultTransitAnima's whitelist — admin-plane peers can
        // pass arbitrary strings; the policy is closed-by-default but
        // we still validate at the RPC boundary so the key store never
        // sees an unsanitised `user_id`. `[a-zA-Z0-9_-]{1,64}`.
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

    fn parse_digest(bytes: &[u8]) -> Result<[u8; 32], Status> {
        if bytes.len() != 32 {
            return Err(Status::invalid_argument(format!(
                "digest must be 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(bytes);
        Ok(out)
    }
}

#[tonic::async_trait]
impl<S: CustodyKeyStore> oracle_pb::custody_oracle_server::CustodyOracle
    for CustodyOracleService<S>
{
    async fn sign_auth(
        &self,
        req: Request<oracle_pb::SignAuthRequest>,
    ) -> Result<Response<oracle_pb::SignAuthResponse>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SignAuth)?;
        let inner = req.into_inner();
        Self::validate_user_id(&inner.user_id)?;
        let digest = Self::parse_digest(&inner.digest)?;
        let sig = self.store.sign_auth_digest(&inner.user_id, &digest)?;
        Ok(Response::new(oracle_pb::SignAuthResponse {
            signature_raw: sig.to_vec(),
        }))
    }

    async fn sign_wallet(
        &self,
        req: Request<oracle_pb::SignWalletRequest>,
    ) -> Result<Response<oracle_pb::SignWalletResponse>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SignWallet)?;
        let inner = req.into_inner();
        Self::validate_user_id(&inner.user_id)?;
        let digest = Self::parse_digest(&inner.digest)?;
        let sig = self.store.sign_wallet_digest(&inner.user_id, &digest)?;
        Ok(Response::new(oracle_pb::SignWalletResponse {
            signature_rsv: sig.to_vec(),
        }))
    }

    async fn get_auth_pubkey(
        &self,
        req: Request<oracle_pb::GetAuthPubkeyRequest>,
    ) -> Result<Response<oracle_pb::GetAuthPubkeyResponse>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::GetAuthPubkey)?;
        let inner = req.into_inner();
        Self::validate_user_id(&inner.user_id)?;
        let pk = self.store.auth_pubkey_sec1(&inner.user_id)?;
        Ok(Response::new(oracle_pb::GetAuthPubkeyResponse {
            pubkey_sec1_compressed: pk.to_vec(),
        }))
    }

    async fn get_wallet_pubkey(
        &self,
        req: Request<oracle_pb::GetWalletPubkeyRequest>,
    ) -> Result<Response<oracle_pb::GetWalletPubkeyResponse>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::GetWalletPubkey)?;
        let inner = req.into_inner();
        Self::validate_user_id(&inner.user_id)?;
        let pk = self.store.wallet_pubkey_sec1_uncompressed(&inner.user_id)?;
        Ok(Response::new(oracle_pb::GetWalletPubkeyResponse {
            pubkey_sec1_uncompressed: pk.to_vec(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::policy::AdminPolicy;
    use life_kernel_proto::custody::custody_oracle_server::CustodyOracle as _;

    fn fixture_service() -> CustodyOracleService<InProcessCustodyKeys> {
        let store = InProcessCustodyKeys::new();
        store.insert_user("alice", [7u8; 32], [11u8; 32]);
        CustodyOracleService::new(Arc::new(AdminPolicy::permissive()), Arc::new(store))
    }

    fn make_request<T>(value: T, cred: PeerCred) -> Request<T> {
        let mut req = Request::new(value);
        req.extensions_mut().insert(AdminConnInfo { cred });
        req
    }

    #[tokio::test]
    async fn sign_auth_happy_path() {
        let svc = fixture_service();
        let req = make_request(
            oracle_pb::SignAuthRequest {
                user_id: "alice".into(),
                digest: vec![42u8; 32],
            },
            PeerCred {
                pid: 0,
                uid: 1000,
                gid: 1000,
            },
        );
        let resp = svc.sign_auth(req).await.unwrap().into_inner();
        assert_eq!(resp.signature_raw.len(), 64);
    }

    #[tokio::test]
    async fn sign_wallet_returns_65_bytes_with_v() {
        let svc = fixture_service();
        let req = make_request(
            oracle_pb::SignWalletRequest {
                user_id: "alice".into(),
                digest: vec![42u8; 32],
            },
            PeerCred {
                pid: 0,
                uid: 1000,
                gid: 1000,
            },
        );
        let resp = svc.sign_wallet(req).await.unwrap().into_inner();
        assert_eq!(resp.signature_rsv.len(), 65);
        let v = resp.signature_rsv[64];
        assert!(v == 27 || v == 28);
    }

    #[tokio::test]
    async fn pubkey_bootstrap_rpcs_are_idempotent() {
        let svc = fixture_service();
        let cred = PeerCred {
            pid: 0,
            uid: 1000,
            gid: 1000,
        };
        let r1 = svc
            .get_auth_pubkey(make_request(
                oracle_pb::GetAuthPubkeyRequest {
                    user_id: "alice".into(),
                },
                cred,
            ))
            .await
            .unwrap()
            .into_inner();
        let r2 = svc
            .get_auth_pubkey(make_request(
                oracle_pb::GetAuthPubkeyRequest {
                    user_id: "alice".into(),
                },
                cred,
            ))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r1.pubkey_sec1_compressed, r2.pubkey_sec1_compressed);
        assert_eq!(r1.pubkey_sec1_compressed.len(), 33);
    }

    #[tokio::test]
    async fn user_id_validation_rejects_path_traversal() {
        let svc = fixture_service();
        let req = make_request(
            oracle_pb::SignAuthRequest {
                user_id: "alice/../admin".into(),
                digest: vec![0u8; 32],
            },
            PeerCred {
                pid: 0,
                uid: 1000,
                gid: 1000,
            },
        );
        let err = svc.sign_auth(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn missing_user_returns_not_found() {
        let svc = fixture_service();
        let req = make_request(
            oracle_pb::SignAuthRequest {
                user_id: "nobody".into(),
                digest: vec![0u8; 32],
            },
            PeerCred {
                pid: 0,
                uid: 1000,
                gid: 1000,
            },
        );
        let err = svc.sign_auth(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn missing_peer_cred_returns_internal() {
        // Manually build a request without AdminConnInfo extension —
        // soma's acceptor always populates it; this test guards against
        // a regression where an alternate transport forgets to.
        let svc = fixture_service();
        let req = Request::new(oracle_pb::SignAuthRequest {
            user_id: "alice".into(),
            digest: vec![0u8; 32],
        });
        let err = svc.sign_auth(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    #[tokio::test]
    async fn strict_policy_rejects_stranger() {
        let store = InProcessCustodyKeys::new();
        store.insert_user("alice", [7u8; 32], [11u8; 32]);
        let svc = CustodyOracleService::new(Arc::new(AdminPolicy::strict(1500)), Arc::new(store));
        let req = make_request(
            oracle_pb::SignAuthRequest {
                user_id: "alice".into(),
                digest: vec![0u8; 32],
            },
            PeerCred {
                pid: 0,
                uid: 1,
                gid: 1,
            },
        );
        let err = svc.sign_auth(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }
}
