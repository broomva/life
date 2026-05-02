//! Admin plane for lifegw. Sub-phase D (D2).
//!
//! Mounts the `life.admin.gw.v1.GatewayAdmin` service on a UDS at
//! `/run/life/lifegw-admin.sock` (group `life-admin`, mode `0660`).
//! Authn is SO_PEERCRED + group membership — NO bearer tokens (the
//! prompt's hard rule).
//!
//! Exposes:
//! - `HealthCheck` — liveness + version + JWKS metadata (anyone).
//! - `CertReload` — SIGHUP-equivalent cert reload over RPC (D3).
//! - `JwksDump` — read-only JWKS cache view.
//! - `Blocklist_{Add,Remove,List}` — in-memory IP/user blocklist.
//! - `RateLimit_Override` — per-user QPS runtime override.
//!
//! All ops except `HealthCheck` require either `admin_gid` group
//! membership OR root.

pub mod blocklist;
pub mod listener;
pub mod metrics;
pub mod peercred;
pub mod policy;
pub mod service;

pub use blocklist::{Blocklist, BlocklistEntry};
pub use listener::{AdminAcceptor, AdminConn, AdminConnInfo};
pub use metrics::AdminMetrics;
pub use peercred::{PeerCred, group_gid, is_member_of, peer_cred};
pub use policy::{AdminOp, AdminPolicy};
pub use service::{CertReloadHook, CertReloadOutcome, GatewayAdminService};
