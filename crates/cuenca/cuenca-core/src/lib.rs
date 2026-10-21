//! The Cuenca authority's state (spec `docs/specs/2026-10-19-agent-native-change-plane.html`
//! in broomva/workspace, sections 2a, 3 and 6b).
//!
//! A [`Store`] is one directory holding:
//!
//! - **the log**, an append-only sequence of [`Record`]s in a redb database. Each call
//!   that changes it is one write transaction that runs the event through the
//!   transition rules (one copy of them, in `state.rs`), appends exactly one record and
//!   updates the projections, so a crash leaves the record and its effects, or neither;
//!   commits are durable before they return;
//! - **projections** of the log: [`Location`]s (any directory tree the authority tracks,
//!   with an optional git remote), [`Fork`]s with their head, and a changeset index.
//!   [`Store::replay_check`] replays the log through the same transition rules and
//!   compares (admission checks that need blob content are the writer's alone). The
//!   git tree objects every recorded tree is made of sit beside them, checked against
//!   their id on every read;
//! - **blobs**, keyed by git object id. A blob enters the store only after its bytes
//!   hash to the claimed id ([`cuenca_tree::BlobHasher`], with SHA-1 collision
//!   detection), and its SHA-256 is derived from those verified bytes.
//!
//! Who may do what (holders, fencing epochs, takeover, revocation) is BRO-3249, built
//! on top of this layer; this layer only enforces that the log stays well-formed.
//!
//! # Why not `lago-journal`
//!
//! `lago-journal` stores Lago's `EventEnvelope`s keyed by session, branch and sequence,
//! behind an async `Journal` trait. The authority needs a single global order, one
//! transaction covering an append and its projections, and synchronous calls. It uses the
//! same engine, redb, directly.

mod blobs;
mod error;
mod event;
mod names;
mod state;
mod store;

pub use blobs::MAX_BLOB_BYTES;
pub use error::{CoreError, Result};
pub use event::{ChangesetId, Event, Record};
pub use names::{ForkName, LocationName, MAX_NAME_BYTES, NameError};
pub use state::{Fork, Location, MainMove};
pub use store::{BlobInfo, Published, ReplaySummary, Store};
