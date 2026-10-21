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
//! # One writer per fork
//!
//! Every fork has a holder ([`Holder`]: a principal writing from one machine) and a
//! fencing epoch, 1 when the fork is created. Only the holder, under the current epoch,
//! may publish a draft or ask to land. [`Store::take_over`] moves the fork to a new
//! holder by compare-and-swap on the epoch, which it increments, and drops the old
//! holder's landing intent (moving main does not read intents: landing, BRO-3255,
//! must check, in the transaction that moves main, that the intent is live and its
//! holder still holds the fork and is not revoked: a revocation leaves intents in place). A
//! takeover retried after it succeeded gets `EpochMismatch`; read the fork to see who
//! holds it. A publish by the holder of an earlier epoch is not
//! dropped and not merged in: it becomes a draft of that epoch's sibling fork,
//! `<fork>-from-<node>-e<epoch>` ([`sibling_name`]), one level further down if that
//! sibling was taken over too. A sibling starts from the head its epoch ended at and
//! from no other tree, so a stale writer never gains a tree it was not given (another
//! location's main, another fork's head); a stale draft that does not continue that
//! head is refused as `NotAtBase` and stays in the writer's checkout. [`Store::revoke`] makes the
//! log refuse every later write a principal asks for. Who a request comes from (keys,
//! `tailscale whois`) is the authority's to establish (BRO-3251); this layer trusts
//! the [`Writer`] it is given.
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
mod owner;
mod state;
mod store;

pub use blobs::MAX_BLOB_BYTES;
pub use error::{CoreError, Result};
pub use event::{ChangesetId, Event, Record};
pub use names::{ForkName, LocationName, MAX_NAME_BYTES, NameError, PrincipalName};
pub use owner::{Holder, MAX_NODE_BYTES, NodeId, Writer, sibling_name};
pub use state::{Fork, Location, MainMove};
pub use store::{BlobInfo, Published, ReplaySummary, Store};
