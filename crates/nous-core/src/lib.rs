//! Nous core — types, traits, and errors for metacognitive evaluation.
//!
//! This crate defines the vocabulary used across all Nous components.
//! It has zero I/O and depends only on `aios-protocol`.

pub mod egri;
pub mod error;
pub mod evaluator;
pub mod events;
pub mod registry;
pub mod score;
pub mod taxonomy;

// Re-exports for convenience.
pub use error::{NousError, NousResult};
pub use evaluator::{EvalContext, EvalHook, NousEvaluator};
pub use events::{EVAL_EVENT_PREFIX, NousEvent};
pub use registry::EvaluatorRegistry;
pub use score::{EvalResult, EvalScore, ScoreLabel};
pub use taxonomy::{EvalLayer, EvalTiming};
