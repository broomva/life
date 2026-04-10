//! Evaluator trait — the core abstraction for all Nous evaluators.
//!
//! Evaluators are pure functions: given an `EvalContext`, they produce
//! zero or more `EvalScore`s. Inline evaluators must complete in < 2ms.

use crate::error::NousResult;
use crate::score::EvalScore;
use crate::taxonomy::{EvalLayer, EvalTiming};
use serde::{Deserialize, Serialize};

/// Context provided to evaluators for scoring.
///
/// Carries the information an evaluator needs without requiring
/// it to depend on Arcan types directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalContext {
    /// Session ID.
    pub session_id: String,
    /// Run ID within the session.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Current iteration within the run.
    #[serde(default)]
    pub iteration: Option<u32>,
    /// Input token count for the current model call.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Output token count for the current model call.
    #[serde(default)]
    pub output_tokens: Option<u64>,
    /// Remaining token budget.
    #[serde(default)]
    pub tokens_remaining: Option<u64>,
    /// Total tokens used so far in the session.
    #[serde(default)]
    pub total_tokens_used: Option<u64>,
    /// Number of tool calls in this run.
    #[serde(default)]
    pub tool_call_count: Option<u32>,
    /// Number of tool errors in this run.
    #[serde(default)]
    pub tool_error_count: Option<u32>,
    /// Tool name (for tool-specific evaluators).
    #[serde(default)]
    pub tool_name: Option<String>,
    /// Whether the tool call resulted in an error.
    #[serde(default)]
    pub tool_errored: Option<bool>,
    /// Maximum iterations configured.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Knowledge coverage at evaluation time (0.0-1.0). None if unavailable.
    #[serde(default)]
    pub knowledge_coverage: Option<f64>,
    /// Knowledge freshness at evaluation time (0.0-1.0). None if unavailable.
    #[serde(default)]
    pub knowledge_freshness: Option<f64>,
    /// Number of knowledge notes retrieved during this run.
    #[serde(default)]
    pub knowledge_retrieved_count: Option<u32>,
    /// Top relevance score from the most recent knowledge search.
    #[serde(default)]
    pub knowledge_top_relevance: Option<f64>,
    /// Most recent knowledge search query for correlation.
    #[serde(default)]
    pub knowledge_query: Option<String>,
    /// Arbitrary key-value metadata.
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

impl EvalContext {
    /// Create a minimal context with just a session ID.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            run_id: None,
            iteration: None,
            input_tokens: None,
            output_tokens: None,
            tokens_remaining: None,
            total_tokens_used: None,
            tool_call_count: None,
            tool_error_count: None,
            tool_name: None,
            tool_errored: None,
            max_iterations: None,
            knowledge_coverage: None,
            knowledge_freshness: None,
            knowledge_retrieved_count: None,
            knowledge_top_relevance: None,
            knowledge_query: None,
            metadata: std::collections::HashMap::new(),
        }
    }
}

/// The core evaluator trait.
///
/// All Nous evaluators implement this trait. Inline evaluators
/// must be fast (< 2ms, no I/O). Async evaluators may take longer.
pub trait NousEvaluator: Send + Sync {
    /// Unique name for this evaluator (e.g. `token_efficiency`).
    fn name(&self) -> &str;

    /// Which behavior layer this evaluator measures.
    fn layer(&self) -> EvalLayer;

    /// Whether this runs inline or async.
    fn timing(&self) -> EvalTiming;

    /// Evaluate the given context and produce scores.
    ///
    /// Returns an empty vec if there's insufficient data to score.
    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>>;
}

/// Hook points where evaluators can be attached in the agent lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvalHook {
    /// Before a model call is made.
    BeforeModelCall,
    /// After a model call completes.
    AfterModelCall,
    /// Before a tool call is executed.
    PreToolCall,
    /// After a tool call completes.
    PostToolCall,
    /// After a full run finishes.
    OnRunFinished,
}

impl EvalHook {
    /// String representation for logging and events.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BeforeModelCall => "before_model_call",
            Self::AfterModelCall => "after_model_call",
            Self::PreToolCall => "pre_tool_call",
            Self::PostToolCall => "post_tool_call",
            Self::OnRunFinished => "on_run_finished",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taxonomy::{EvalLayer, EvalTiming};

    struct MockEvaluator;

    impl NousEvaluator for MockEvaluator {
        fn name(&self) -> &str {
            "mock"
        }

        fn layer(&self) -> EvalLayer {
            EvalLayer::Execution
        }

        fn timing(&self) -> EvalTiming {
            EvalTiming::Inline
        }

        fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
            let score = EvalScore::new(
                self.name(),
                0.9,
                self.layer(),
                self.timing(),
                &ctx.session_id,
            )?;
            Ok(vec![score])
        }
    }

    #[test]
    fn mock_evaluator_produces_score() {
        let evaluator = MockEvaluator;
        let ctx = EvalContext::new("sess-1");
        let scores = evaluator.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].evaluator, "mock");
        assert!((scores[0].value - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_context_new_minimal() {
        let ctx = EvalContext::new("test");
        assert_eq!(ctx.session_id, "test");
        assert!(ctx.run_id.is_none());
        assert!(ctx.input_tokens.is_none());
        assert!(ctx.knowledge_coverage.is_none());
        assert!(ctx.knowledge_query.is_none());
    }

    #[test]
    fn eval_context_serde_defaults_new_knowledge_fields() {
        let json = r#"{"session_id":"sess-1","metadata":{}}"#;
        let ctx: EvalContext = serde_json::from_str(json).unwrap();
        assert_eq!(ctx.session_id, "sess-1");
        assert!(ctx.knowledge_coverage.is_none());
        assert!(ctx.knowledge_freshness.is_none());
        assert!(ctx.knowledge_retrieved_count.is_none());
        assert!(ctx.knowledge_top_relevance.is_none());
        assert!(ctx.knowledge_query.is_none());
    }

    #[test]
    fn eval_hook_as_str() {
        assert_eq!(EvalHook::BeforeModelCall.as_str(), "before_model_call");
        assert_eq!(EvalHook::AfterModelCall.as_str(), "after_model_call");
        assert_eq!(EvalHook::PreToolCall.as_str(), "pre_tool_call");
        assert_eq!(EvalHook::PostToolCall.as_str(), "post_tool_call");
        assert_eq!(EvalHook::OnRunFinished.as_str(), "on_run_finished");
    }
}
