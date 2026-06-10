//! Bookkeeping-judge — Nous-gate promotion judge re-implemented as an
//! [`ergon::Workflow`].
//!
//! This is the surface validation for the ergon harness primitive
//! (Linear BRO-1003 / Ergon v0.1 spec §12.9): a port of the
//! Bellows-shipped `bellows-example-bookkeeping-judge` that keeps the
//! input + output schema identical and swaps the substrate from
//! Bellows runtime to `ergon::Workflow` + authored agents.
//!
//! ## What it does
//!
//! Reads a raw extract markdown file under
//! `research/notes/<date>-*-raw.md`, parses each `## Item N` block,
//! then dispatches every item through three authored agents
//! (`bookkeeping-novelty`, `bookkeeping-specificity`,
//! `bookkeeping-relevance`) and aggregates the per-axis scores into a
//! Nous-gate verdict:
//!
//! - `total = novelty + specificity + relevance` (0..=9)
//! - `pass = total >= 5`
//! - `blog_candidate = total >= 7`
//!
//! ## I/O parity with Bellows reference
//!
//! ```json
//! // input
//! { "extract_path": "/abs/path/to/research/notes/foo-raw.md",
//!   "max_items": 20 }
//!
//! // output
//! { "items": [ { "item_number": 1, "slug": "...", "type": "...",
//!                "novelty": 2, "specificity": 3, "relevance": 3,
//!                "total": 8, "pass": true, "blog_candidate": true,
//!                "reasoning": "..." } ],
//!   "source_file": "...",
//!   "judged_at": "2026-05-20T17:42:11Z",
//!   "provider": "ergon-authored-agents",
//!   "session_id": "..." }
//! ```
//!
//! The output schema is locked — downstream Python tooling
//! (`skills/bookkeeping/scripts/bookkeeping.py`) consumes this JSON and
//! must continue to parse it byte-compatibly.

#![doc(html_no_source)]

pub mod parse;
pub mod score;
pub mod workflow;

pub use score::{ItemKind, JudgedItem, RawItem, aggregate};
pub use workflow::{BookkeepingJudge, JudgeInput, JudgeOutput};
