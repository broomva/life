//! `NousMiddleware` — the Arcan middleware that runs Nous evaluators.
//!
//! Implements `arcan_core::runtime::Middleware` to run inline evaluators
//! at each hook point in the agent lifecycle. Scores are logged via tracing
//! and accumulated for downstream consumers (Vigil, Autonomic, Lago).

use std::sync::{Arc, Mutex};

use arcan_core::error::CoreError;
use arcan_core::protocol::{ModelTurn, ToolResult};
use arcan_core::runtime::{Middleware, ProviderRequest, RunOutput, ToolContext};
use nous_core::{EvalContext, EvalHook, EvalScore, EvaluatorRegistry};
use tracing::{debug, warn};

/// Accumulated eval state for a middleware instance.
#[derive(Debug, Default)]
struct EvalAccumulator {
    scores: Vec<EvalScore>,
    tool_call_count: u32,
    tool_error_count: u32,
}

/// Callback type for score notifications.
type ScoreCallback = Arc<dyn Fn(&EvalScore) + Send + Sync>;

/// Arcan middleware that runs Nous evaluators at each hook point.
///
/// Created with a populated `EvaluatorRegistry` and runs the appropriate
/// evaluators at each lifecycle hook. Scores are accumulated and can be
/// retrieved after a run completes.
pub struct NousMiddleware {
    registry: EvaluatorRegistry,
    accumulator: Mutex<EvalAccumulator>,
    /// Callback invoked for each score produced (for Vigil/Lago integration).
    on_score: Option<ScoreCallback>,
}

impl NousMiddleware {
    /// Create a new middleware with the given evaluator registry.
    pub fn new(registry: EvaluatorRegistry) -> Self {
        Self {
            registry,
            accumulator: Mutex::new(EvalAccumulator::default()),
            on_score: None,
        }
    }

    /// Create a middleware with a score callback.
    pub fn with_on_score(registry: EvaluatorRegistry, on_score: ScoreCallback) -> Self {
        Self {
            registry,
            accumulator: Mutex::new(EvalAccumulator::default()),
            on_score: Some(on_score),
        }
    }

    /// Create a middleware with the default heuristic evaluators.
    pub fn with_defaults() -> Result<Self, nous_core::NousError> {
        let registry = nous_heuristics::default_registry()?;
        Ok(Self::new(registry))
    }

    /// Number of registered evaluators.
    pub fn registry_len(&self) -> usize {
        self.registry.len()
    }

    /// Get all accumulated scores from evaluations in this middleware instance.
    pub fn scores(&self) -> Vec<EvalScore> {
        self.accumulator
            .lock()
            .expect("accumulator lock poisoned")
            .scores
            .clone()
    }

    /// Run evaluators for a given hook and context, accumulating scores.
    fn run_evaluators(&self, hook: EvalHook, ctx: &EvalContext) {
        for evaluator in self.registry.evaluators_for(hook) {
            match evaluator.evaluate(ctx) {
                Ok(scores) => {
                    for score in &scores {
                        debug!(
                            evaluator = score.evaluator,
                            value = score.value,
                            label = score.label.as_str(),
                            layer = %score.layer,
                            hook = hook.as_str(),
                            "nous eval score"
                        );

                        // Emit Vigil evaluation span event for OTel export.
                        life_vigil::spans::eval_event(
                            &score.evaluator,
                            score.value,
                            score.label.as_str(),
                            &score.layer.to_string(),
                            "inline",
                        );

                        if let Some(ref cb) = self.on_score {
                            cb(score);
                        }
                    }
                    if let Ok(mut acc) = self.accumulator.lock() {
                        acc.scores.extend(scores);
                    }
                }
                Err(e) => {
                    warn!(
                        evaluator = evaluator.name(),
                        error = %e,
                        hook = hook.as_str(),
                        "nous evaluator failed"
                    );
                }
            }
        }
    }

    /// Build an `EvalContext` from a `ProviderRequest`.
    fn ctx_from_request(&self, request: &ProviderRequest) -> EvalContext {
        let mut ctx = EvalContext::new(&request.session_id);
        ctx.run_id = Some(request.run_id.clone());
        ctx.iteration = Some(request.iteration);
        ctx
    }

    /// Build an `EvalContext` from a `ProviderRequest` + `ModelTurn`.
    fn ctx_from_response(&self, request: &ProviderRequest, response: &ModelTurn) -> EvalContext {
        let mut ctx = self.ctx_from_request(request);
        if let Some(ref usage) = response.usage {
            ctx.input_tokens = Some(usage.input_tokens);
            ctx.output_tokens = Some(usage.output_tokens);
        }
        ctx
    }
}

impl Middleware for NousMiddleware {
    fn before_model_call(&self, request: &ProviderRequest) -> Result<(), CoreError> {
        let ctx = self.ctx_from_request(request);
        self.run_evaluators(EvalHook::BeforeModelCall, &ctx);
        Ok(())
    }

    fn after_model_call(
        &self,
        request: &ProviderRequest,
        response: &ModelTurn,
    ) -> Result<(), CoreError> {
        let ctx = self.ctx_from_response(request, response);
        self.run_evaluators(EvalHook::AfterModelCall, &ctx);
        Ok(())
    }

    fn pre_tool_call(
        &self,
        context: &ToolContext,
        call: &arcan_core::protocol::ToolCall,
    ) -> Result<(), CoreError> {
        let mut ctx = EvalContext::new(&context.session_id);
        ctx.run_id = Some(context.run_id.clone());
        ctx.iteration = Some(context.iteration);
        ctx.tool_name = Some(call.tool_name.clone());
        self.run_evaluators(EvalHook::PreToolCall, &ctx);
        Ok(())
    }

    fn post_tool_call(&self, context: &ToolContext, result: &ToolResult) -> Result<(), CoreError> {
        // Update tool counters.
        if let Ok(mut acc) = self.accumulator.lock() {
            acc.tool_call_count += 1;
            if result.is_error {
                acc.tool_error_count += 1;
            }
        }

        let mut ctx = EvalContext::new(&context.session_id);
        ctx.run_id = Some(context.run_id.clone());
        ctx.iteration = Some(context.iteration);
        ctx.tool_name = Some(result.tool_name.clone());
        ctx.tool_errored = Some(result.is_error);

        self.run_evaluators(EvalHook::PostToolCall, &ctx);
        Ok(())
    }

    fn on_run_finished(&self, output: &RunOutput) -> Result<(), CoreError> {
        let acc = self.accumulator.lock().expect("accumulator lock poisoned");

        let mut ctx = EvalContext::new(&output.session_id);
        ctx.run_id = Some(output.run_id.clone());
        ctx.tool_call_count = Some(acc.tool_call_count);
        ctx.tool_error_count = Some(acc.tool_error_count);
        ctx.input_tokens = Some(output.total_usage.input_tokens);
        ctx.output_tokens = Some(output.total_usage.output_tokens);

        // Find max_iterations from run events (look for RunStarted).
        if let Some(arcan_core::protocol::AgentEvent::RunStarted { max_iterations, .. }) =
            output.events.first()
        {
            ctx.max_iterations = Some(*max_iterations);
        }

        // Determine current iteration from events count.
        let iteration_count = output
            .events
            .iter()
            .filter(|e| matches!(e, arcan_core::protocol::AgentEvent::IterationStarted { .. }))
            .count() as u32;
        ctx.iteration = Some(iteration_count);

        // Release lock before running evaluators (they may try to accumulate).
        drop(acc);

        self.run_evaluators(EvalHook::OnRunFinished, &ctx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arcan_core::protocol::{AgentEvent, RunStopReason, TokenUsage};
    use arcan_core::state::AppState;

    #[test]
    fn middleware_with_defaults_creates() {
        let mw = NousMiddleware::with_defaults().unwrap();
        assert!(!mw.registry.is_empty());
    }

    #[test]
    fn middleware_accumulates_scores_on_after_model() {
        let mw = NousMiddleware::with_defaults().unwrap();

        let request = ProviderRequest {
            run_id: "run-1".into(),
            session_id: "sess-1".into(),
            iteration: 1,
            messages: vec![],
            tools: vec![],
            state: AppState::default(),
        };
        let response = ModelTurn {
            directives: vec![],
            stop_reason: arcan_core::protocol::ModelStopReason::EndTurn,
            usage: Some(TokenUsage {
                input_tokens: 1000,
                output_tokens: 200,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
        };

        let result = mw.after_model_call(&request, &response);
        assert!(result.is_ok());

        let scores = mw.scores();
        // TokenEfficiency + BudgetAdherence should produce scores
        // (BudgetAdherence won't fire — no budget data in the response context)
        assert!(
            !scores.is_empty(),
            "should have at least one score from token_efficiency"
        );
    }

    #[test]
    fn middleware_tracks_tool_calls() {
        let mw = NousMiddleware::with_defaults().unwrap();

        let context = ToolContext {
            run_id: "run-1".into(),
            session_id: "sess-1".into(),
            iteration: 1,
        };
        let result = ToolResult {
            call_id: "c1".into(),
            tool_name: "read_file".into(),
            output: serde_json::json!({"content": "hello"}),
            content: None,
            is_error: false,
            state_patch: None,
        };

        mw.post_tool_call(&context, &result).unwrap();

        let acc = mw.accumulator.lock().unwrap();
        assert_eq!(acc.tool_call_count, 1);
        assert_eq!(acc.tool_error_count, 0);
    }

    #[test]
    fn middleware_tracks_tool_errors() {
        let mw = NousMiddleware::with_defaults().unwrap();

        let context = ToolContext {
            run_id: "run-1".into(),
            session_id: "sess-1".into(),
            iteration: 1,
        };
        let result = ToolResult {
            call_id: "c1".into(),
            tool_name: "write_file".into(),
            output: serde_json::json!({"error": "permission denied"}),
            content: None,
            is_error: true,
            state_patch: None,
        };

        mw.post_tool_call(&context, &result).unwrap();

        let acc = mw.accumulator.lock().unwrap();
        assert_eq!(acc.tool_call_count, 1);
        assert_eq!(acc.tool_error_count, 1);
    }

    #[test]
    fn middleware_on_run_finished_fires_evaluators() {
        let mw = NousMiddleware::with_defaults().unwrap();

        // Simulate some tool calls first.
        {
            let mut acc = mw.accumulator.lock().unwrap();
            acc.tool_call_count = 5;
            acc.tool_error_count = 1;
        }

        let output = RunOutput {
            run_id: "run-1".into(),
            session_id: "sess-1".into(),
            branch_id: "main".into(),
            events: vec![
                AgentEvent::RunStarted {
                    run_id: "run-1".into(),
                    session_id: "sess-1".into(),
                    provider: "mock".into(),
                    max_iterations: 24,
                },
                AgentEvent::IterationStarted {
                    run_id: "run-1".into(),
                    session_id: "sess-1".into(),
                    iteration: 1,
                },
                AgentEvent::IterationStarted {
                    run_id: "run-1".into(),
                    session_id: "sess-1".into(),
                    iteration: 2,
                },
                AgentEvent::RunFinished {
                    run_id: "run-1".into(),
                    session_id: "sess-1".into(),
                    reason: RunStopReason::Completed,
                    total_iterations: 2,
                    final_answer: Some("done".into()),
                    usage: Some(TokenUsage {
                        input_tokens: 500,
                        output_tokens: 200,
                        cache_read_tokens: 0,
                        cache_creation_tokens: 0,
                    }),
                },
            ],
            messages: vec![],
            state: AppState::default(),
            reason: RunStopReason::Completed,
            final_answer: Some("done".into()),
            total_usage: TokenUsage {
                input_tokens: 500,
                output_tokens: 200,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        };

        let result = mw.on_run_finished(&output);
        assert!(result.is_ok());

        let scores = mw.scores();
        // Should have tool_correctness + step_efficiency scores from on_run_finished.
        let run_finished_scores: Vec<_> = scores
            .iter()
            .filter(|s| s.evaluator == "tool_correctness" || s.evaluator == "step_efficiency")
            .collect();
        assert!(
            run_finished_scores.len() >= 2,
            "expected tool_correctness and step_efficiency scores, got {:?}",
            run_finished_scores
                .iter()
                .map(|s| &s.evaluator)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn on_score_callback_fires() {
        let score_count = Arc::new(Mutex::new(0u32));
        let counter = score_count.clone();

        let registry = nous_heuristics::default_registry().unwrap();
        let mw = NousMiddleware::with_on_score(
            registry,
            Arc::new(move |_score| {
                *counter.lock().unwrap() += 1;
            }),
        );

        let request = ProviderRequest {
            run_id: "run-1".into(),
            session_id: "sess-1".into(),
            iteration: 1,
            messages: vec![],
            tools: vec![],
            state: AppState::default(),
        };
        let response = ModelTurn {
            directives: vec![],
            stop_reason: arcan_core::protocol::ModelStopReason::EndTurn,
            usage: Some(TokenUsage {
                input_tokens: 1000,
                output_tokens: 200,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
        };

        mw.after_model_call(&request, &response).unwrap();

        let count = *score_count.lock().unwrap();
        assert!(count > 0, "callback should have fired at least once");
    }
}
