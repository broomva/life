//! # nous-tools — Lineage primitives + praxis tools for authored agents
//!
//! This crate ships the substrate layer (L2 in the authored-agents
//! architecture) for nous-driven workflows:
//!
//! - [`NousLineage`] trait + [`InMemoryNousLineage`] reference impl —
//!   a record/query interface for the provenance of every nous decision.
//!   The lago-backed implementation is intentionally out of scope; this
//!   crate ships the trait and an in-memory fixture so authored agents
//!   (BRO-1011 nous-promoter, BRO-1012 bookkeeping scorers) can be wired
//!   end-to-end without taking a dependency on the journal.
//! - Three [`praxis`-style](aios_protocol::tool::Tool) tools that
//!   authored agents need to consume nous output:
//!   - [`NousAggregateTool`] — descriptive statistics over a `Vec<f64>`
//!     of scores (count, mean, median, min, max, stddev).
//!   - [`NousCompareTool`] — `a` vs `b` with a configurable equality
//!     threshold; emits a typed verdict.
//!   - [`LagoQueryTool`] — read-only event-journal queries over
//!     [`lago_core::Journal`] with kind / time / limit filters.
//!
//! These primitives are referenced as the BRO-1009 row of the
//! authored-agents architecture spec
//! (`docs/superpowers/specs/2026-05-09-bro-1006-authored-agents-architecture.md`,
//! §6.6 + §8). Lago-backed `NousLineage` and the BRO-1011 `nous-promoter`
//! agent that consumes these tools live downstream.

pub mod aggregate;
pub mod compare;
pub mod error;
pub mod lago_query;
pub mod lineage;

pub use aggregate::{AggregateOutput, NousAggregateTool};
pub use compare::{CompareOutput, CompareVerdict, NousCompareTool};
pub use error::NousLineageError;
pub use lago_query::{LagoQueryTool, QueriedEvent, QueryOutput};
pub use lineage::{InMemoryNousLineage, LineageEvent, LineageFilter, LineageId, NousLineage};
