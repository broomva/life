//! Async LLM-as-judge evaluators for Nous.
//!
//! These evaluators run asynchronously after agent runs complete.
//! They use a separate model call to assess quality dimensions
//! that require language understanding.

pub mod judge_provider;
pub mod plan_adherence;
pub mod plan_quality;
pub mod task_completion;

pub use judge_provider::{JudgeProvider, MockJudgeProvider, parse_judge_scores};
pub use plan_adherence::PlanAdherence;
pub use plan_quality::PlanQuality;
pub use task_completion::TaskCompletion;
