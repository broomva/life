//! Spec D D-Sub-E — soma admin custody-oracle UDS module.
//!
//! Hosts the `life.admin.kernel.v1.CustodyOracle` service on a separate
//! Unix domain socket from the kernel µVM service. Authn is
//! SO_PEERCRED + group membership (`life-runtime`); NO bearer tokens.
//!
//! See `crates/life-kernel/CLAUDE.md` and the Spec D phasing section
//! for the architectural framing — this is the soma side of the
//! user-scoped Tier-2 KMS unification (sibling of lifegw's Tier-2
//! KMS in Spec C₃ §5).

pub mod keys;
pub mod listener;
pub mod peercred;
pub mod policy;
pub mod service;

pub use keys::{InProcessCustodyKeys, derive_wallet_address};
pub use listener::{AdminAcceptor, AdminConn, AdminConnInfo, bind};
pub use peercred::{PeerCred, group_gid, peer_cred, supplementary_gids_of_uid};
pub use policy::{AdminOp, AdminPolicy};
pub use service::{CustodyKeyStore, CustodyOracleService};

use std::sync::Arc;

use life_kernel_proto::custody as oracle_pb;
use tokio::task::JoinHandle;

use crate::config::SomaConfig;
use crate::error::{SomaError, SomaResult};

/// Spawn the admin custody-oracle UDS.
///
/// Reads `cfg.admin_plane` (REQUIRED — caller must check `is_some()`
/// before invoking this), binds the configured Unix socket, and runs
/// the tonic server in a background task. The returned `JoinHandle`
/// is held by `main.rs` for the daemon lifetime; aborting it on
/// shutdown lets the kernel listener drain unblocked.
///
/// The key store starts empty — operators provision keys via a future
/// management RPC (out of scope for D-Sub-E, which establishes the
/// wire surface). Tests provision keys directly through
/// [`InProcessCustodyKeys::insert_user`].
pub async fn run_admin_plane(cfg: &SomaConfig) -> SomaResult<JoinHandle<()>> {
    let admin_cfg = cfg
        .admin_plane
        .as_ref()
        .ok_or_else(|| SomaError::Config("admin_plane requested but not configured".into()))?;

    // Resolve the admin GID from the configured group name, if any.
    // Production: `unix_socket_group = "life-runtime"` → strict policy
    // bound to that GID. Test/dev (no group): permissive policy.
    let policy = match admin_cfg.unix_socket_group.as_deref() {
        Some(group_name) => match peercred::group_gid(group_name) {
            Ok(Some(gid)) => AdminPolicy::strict(gid),
            Ok(None) => {
                tracing::warn!(
                    target: "soma::admin",
                    group = group_name,
                    "admin group not found — falling back to permissive policy",
                );
                AdminPolicy::permissive()
            }
            Err(e) => {
                tracing::warn!(
                    target: "soma::admin",
                    group = group_name,
                    error = %e,
                    "admin group lookup failed — falling back to permissive policy",
                );
                AdminPolicy::permissive()
            }
        },
        None => AdminPolicy::permissive(),
    };

    let store = Arc::new(InProcessCustodyKeys::new());
    let svc = CustodyOracleService::new(Arc::new(policy), Arc::clone(&store));
    let acceptor = listener::bind(admin_cfg).await?;

    let handle = tokio::spawn(async move {
        let server = oracle_pb::custody_oracle_server::CustodyOracleServer::new(svc);
        let res = tonic::transport::Server::builder()
            .add_service(server)
            .serve_with_incoming(acceptor)
            .await;
        if let Err(e) = res {
            tracing::warn!(target: "soma::admin", error = %e, "admin plane exited with error");
        }
    });

    tracing::info!(
        target: "soma::admin",
        socket = %admin_cfg.unix_socket.display(),
        "soma admin custody-oracle UDS bound",
    );

    Ok(handle)
}
