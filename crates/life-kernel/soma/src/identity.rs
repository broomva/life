//! BRO-1019 — soma identity substrate module.
//!
//! Hosts the `anima.v1.IdentitySubstrate` service alongside
//! `life.admin.kernel.v1.CustodyOracle` on soma's admin plane UDS.
//! Authn is SO_PEERCRED + group membership (`life-runtime`); NO bearer
//! tokens — same model as [`crate::admin::CustodyOracleService`].
//!
//! Why soma (and not a new `animad` daemon):
//!
//! 1. soma is already the kernel daemon hosting anima-related work
//!    (Spec D Anima Custody — `CustodyOracle` service over UDS).
//! 2. `CustodyOracle` (crypto/signing) and `IdentitySubstrate`
//!    (identity-data: accounts, profiles, sessions) are **disjoint
//!    surfaces** — no overlap, but they cohabit the anima module.
//! 3. Avoids spinning up another daemon + systemd unit + dep graph.
//! 4. Pattern: soma's `admin/service.rs` already shows how to add a
//!    tonic service to soma's UDS router.
//!
//! Phase 4 of the close-out for the Topology B substrate-stub gap
//! audit (BRO-1019). Sibling of `crates/arcan/arcand/src/substrate.rs`
//! (BRO-1016), `crates/lago/lagod/src/substrate.rs` (BRO-1017),
//! `crates/haima/haimad/src/substrate.rs` (BRO-1018). After this
//! merge, all 4 `*-proxy` crates have real method bodies talking to
//! real substrate daemon servers.
//!
//! Scope explicitly out of scope:
//!
//! - Persistent storage. The in-memory `IdentityState` lives for the
//!   daemon lifetime; a future ticket will wire `anima-lago` so each
//!   mutating RPC also produces an `EventKind::Custom("anima.*", ...)`
//!   lago event. Mirrors haima's Phase F2 / BRO-1018 follow-up shape.
//! - Lago event publishing for identity changes. Same future ticket as
//!   above.
//! - `AnimaCustody` changes. Spec D is 100% shipped; this module does
//!   NOT touch `CustodyOracle` except as a server-mount sibling.

pub mod service;
pub mod state;

pub use service::IdentitySubstrateService;
pub use state::{
    AccountRecord, IdentityState, IdentityStateError, IdentityStateResult, ProfileRecord,
    SessionRecord,
};
