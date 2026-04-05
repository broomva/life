//! End-to-end test: `NousMiddleware` wired into Arcan's `Orchestrator`.
//!
//! Uses `ScriptedProvider` to drive a deterministic agent loop
//! and verifies that Nous evaluators fire at the correct hooks
//! and produce expected scores.

use std::sync::{Arc, Mutex};

use arcan_core::error::CoreError;
use arcan_core::protocol::{
    ChatMessage, ModelDirective, ModelStopReason, ModelTurn, RunStopReason, TokenUsage, ToolCall,
    ToolDefinition, ToolResult,
};
use arcan_core::runtime::{
    Middleware, Orchestrator, OrchestratorConfig, Provider, ProviderRequest, RunInput, Tool,
    ToolContext, ToolRegistry,
};
use nous_core::EvalScore;
use nous_middleware::NousMiddleware;
use serde_json::json;

// ── Test helpers (mirrors arcan-core's test infrastructure) ──

struct ScriptedProvider {
    turns: Vec<ModelTurn>,
    cursor: Mutex<usize>,
}

impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn complete(&self, _request: &ProviderRequest) -> Result<ModelTurn, CoreError> {
        let mut cursor = self
            .cursor
            .lock()
            .map_err(|_| CoreError::Provider("lock poisoned".to_string()))?;
        let idx = *cursor;
        let Some(turn) = self.turns.get(idx) else {
            return Err(CoreError::Provider("no scripted turn left".to_string()));
        };
        *cursor += 1;
        Ok(turn.clone())
    }
}

struct EchoTool;

impl Tool for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "echo".to_string(),
            description: "Echoes input".to_string(),
            input_schema: json!({"type": "object", "properties": {"value": {"type": "string"}}}),
            title: None,
            output_schema: None,
            annotations: None,
            category: None,
            tags: Vec::new(),
            timeout_secs: None,
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, CoreError> {
        let value = call.input.get("value").cloned().unwrap_or(json!(null));
        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({ "echo": value }),
            content: None,
            is_error: false,
            state_patch: None,
        })
    }
}

struct FailTool;

impl Tool for FailTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "fail".to_string(),
            description: "Always fails".to_string(),
            input_schema: json!({"type": "object"}),
            title: None,
            output_schema: None,
            annotations: None,
            category: None,
            tags: Vec::new(),
            timeout_secs: None,
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, CoreError> {
        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({"error": "intentional failure"}),
            content: None,
            is_error: true,
            state_patch: None,
        })
    }
}

// ── Tests ──

#[test]
fn e2e_simple_chat_produces_scores() {
    // Simple chat: model returns text, no tool use.
    let nous = Arc::new(NousMiddleware::with_defaults().unwrap());

    let provider = ScriptedProvider {
        turns: vec![ModelTurn {
            directives: vec![ModelDirective::FinalAnswer {
                text: "Hello!".to_string(),
            }],
            stop_reason: ModelStopReason::EndTurn,
            usage: Some(TokenUsage {
                input_tokens: 500,
                output_tokens: 100,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
        }],
        cursor: Mutex::new(0),
    };

    let orchestrator = Orchestrator::new(
        Arc::new(provider),
        ToolRegistry::default(),
        vec![nous.clone() as Arc<dyn Middleware>],
        OrchestratorConfig {
            max_iterations: 4,
            context: None,
            context_compiler: None,
        },
    );

    let output = orchestrator.run(
        RunInput {
            run_id: "run-e2e-1".into(),
            session_id: "sess-e2e-1".into(),
            branch_id: "main".into(),
            messages: vec![ChatMessage::user("Hi")],
            state: Default::default(),
        },
        |_| {},
    );

    assert_eq!(output.reason, RunStopReason::Completed);

    let scores = nous.scores();
    // after_model_call should fire TokenEfficiency (BudgetAdherence won't — no budget context).
    // on_run_finished should fire StepEfficiency (ToolCorrectness won't — no tool calls).
    let evaluator_names: Vec<&str> = scores.iter().map(|s| s.evaluator.as_str()).collect();
    assert!(
        evaluator_names.contains(&"token_efficiency"),
        "expected token_efficiency, got: {evaluator_names:?}"
    );

    // Token efficiency: 100/500 = 0.2 ratio, well below ideal (0.5) → score = 1.0.
    let te_score = scores
        .iter()
        .find(|s| s.evaluator == "token_efficiency")
        .unwrap();
    assert!(
        (te_score.value - 1.0).abs() < f64::EPSILON,
        "token_efficiency should be 1.0 for ratio 0.2, got {}",
        te_score.value
    );
}

#[test]
fn e2e_tool_use_produces_scores() {
    let score_log: Arc<Mutex<Vec<EvalScore>>> = Arc::new(Mutex::new(Vec::new()));
    let log_clone = score_log.clone();

    let registry = nous_heuristics::default_registry().unwrap();
    let nous = Arc::new(NousMiddleware::with_on_score(
        registry,
        Arc::new(move |score| {
            log_clone.lock().unwrap().push(score.clone());
        }),
    ));

    let provider = ScriptedProvider {
        turns: vec![
            // Turn 1: call echo tool.
            ModelTurn {
                directives: vec![ModelDirective::ToolCall {
                    call: ToolCall {
                        call_id: "c1".into(),
                        tool_name: "echo".into(),
                        input: json!({"value": "hello"}),
                    },
                }],
                stop_reason: ModelStopReason::ToolUse,
                usage: Some(TokenUsage {
                    input_tokens: 400,
                    output_tokens: 80,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                }),
            },
            // Turn 2: final answer.
            ModelTurn {
                directives: vec![ModelDirective::FinalAnswer {
                    text: "Done".into(),
                }],
                stop_reason: ModelStopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: 600,
                    output_tokens: 50,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                }),
            },
        ],
        cursor: Mutex::new(0),
    };

    let mut tools = ToolRegistry::default();
    tools.register(EchoTool);

    let orchestrator = Orchestrator::new(
        Arc::new(provider),
        tools,
        vec![nous.clone() as Arc<dyn Middleware>],
        OrchestratorConfig {
            max_iterations: 10,
            context: None,
            context_compiler: None,
        },
    );

    let output = orchestrator.run(
        RunInput {
            run_id: "run-e2e-2".into(),
            session_id: "sess-e2e-2".into(),
            branch_id: "main".into(),
            messages: vec![ChatMessage::user("echo hello")],
            state: Default::default(),
        },
        |_| {},
    );

    assert_eq!(output.reason, RunStopReason::Completed);

    // Check scores from the middleware.
    let scores = nous.scores();
    let evaluator_names: Vec<&str> = scores.iter().map(|s| s.evaluator.as_str()).collect();

    // after_model_call fires twice (2 turns) → token_efficiency × 2.
    let te_count = evaluator_names
        .iter()
        .filter(|n| **n == "token_efficiency")
        .count();
    assert!(
        te_count == 2,
        "expected 2 token_efficiency scores (one per model call), got {te_count}"
    );

    // post_tool_call fires for echo → safety_compliance.
    assert!(
        evaluator_names.contains(&"safety_compliance"),
        "expected safety_compliance from post_tool_call"
    );

    // on_run_finished fires → tool_correctness + step_efficiency.
    assert!(
        evaluator_names.contains(&"tool_correctness"),
        "expected tool_correctness from on_run_finished"
    );
    assert!(
        evaluator_names.contains(&"step_efficiency"),
        "expected step_efficiency from on_run_finished"
    );

    // Tool correctness: 1 call, 0 errors → 1.0.
    let tc = scores
        .iter()
        .find(|s| s.evaluator == "tool_correctness")
        .unwrap();
    assert!(
        (tc.value - 1.0).abs() < f64::EPSILON,
        "tool_correctness should be 1.0 with 0 errors, got {}",
        tc.value
    );

    // Verify on_score callback was also called.
    let callback_scores = score_log.lock().unwrap();
    assert!(
        !callback_scores.is_empty(),
        "on_score callback should have been called"
    );
    assert_eq!(
        callback_scores.len(),
        scores.len(),
        "callback should receive same number of scores as accumulated"
    );
}

#[test]
fn e2e_tool_errors_degrade_scores() {
    let nous = Arc::new(NousMiddleware::with_defaults().unwrap());

    let provider = ScriptedProvider {
        turns: vec![
            // Turn 1: call fail tool.
            ModelTurn {
                directives: vec![ModelDirective::ToolCall {
                    call: ToolCall {
                        call_id: "c1".into(),
                        tool_name: "fail".into(),
                        input: json!({}),
                    },
                }],
                stop_reason: ModelStopReason::ToolUse,
                usage: Some(TokenUsage {
                    input_tokens: 300,
                    output_tokens: 50,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                }),
            },
            // Turn 2: call echo tool (succeeds).
            ModelTurn {
                directives: vec![ModelDirective::ToolCall {
                    call: ToolCall {
                        call_id: "c2".into(),
                        tool_name: "echo".into(),
                        input: json!({"value": "ok"}),
                    },
                }],
                stop_reason: ModelStopReason::ToolUse,
                usage: Some(TokenUsage {
                    input_tokens: 500,
                    output_tokens: 60,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                }),
            },
            // Turn 3: done.
            ModelTurn {
                directives: vec![ModelDirective::FinalAnswer {
                    text: "Recovered".into(),
                }],
                stop_reason: ModelStopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: 700,
                    output_tokens: 30,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                }),
            },
        ],
        cursor: Mutex::new(0),
    };

    let mut tools = ToolRegistry::default();
    tools.register(EchoTool);
    tools.register(FailTool);

    let orchestrator = Orchestrator::new(
        Arc::new(provider),
        tools,
        vec![nous.clone() as Arc<dyn Middleware>],
        OrchestratorConfig {
            max_iterations: 10,
            context: None,
            context_compiler: None,
        },
    );

    let output = orchestrator.run(
        RunInput {
            run_id: "run-e2e-3".into(),
            session_id: "sess-e2e-3".into(),
            branch_id: "main".into(),
            messages: vec![ChatMessage::user("test errors")],
            state: Default::default(),
        },
        |_| {},
    );

    assert_eq!(output.reason, RunStopReason::Completed);

    let scores = nous.scores();

    // safety_compliance should show one error (fail tool) and one success (echo).
    let safety_scores: Vec<&EvalScore> = scores
        .iter()
        .filter(|s| s.evaluator == "safety_compliance")
        .collect();
    assert_eq!(
        safety_scores.len(),
        2,
        "expected 2 safety_compliance scores"
    );
    // First tool (fail) → 0.0, second (echo) → 1.0.
    assert!(
        (safety_scores[0].value).abs() < f64::EPSILON,
        "fail tool should score 0.0"
    );
    assert!(
        (safety_scores[1].value - 1.0).abs() < f64::EPSILON,
        "echo tool should score 1.0"
    );

    // tool_correctness: 2 calls, 1 error → 0.5.
    let tc = scores
        .iter()
        .find(|s| s.evaluator == "tool_correctness")
        .unwrap();
    assert!(
        (tc.value - 0.5).abs() < f64::EPSILON,
        "tool_correctness should be 0.5 with 1/2 errors, got {}",
        tc.value
    );
}

#[test]
fn e2e_verbose_output_degrades_token_efficiency() {
    let nous = Arc::new(NousMiddleware::with_defaults().unwrap());

    let provider = ScriptedProvider {
        turns: vec![ModelTurn {
            directives: vec![ModelDirective::FinalAnswer {
                text: "Very long response".into(),
            }],
            stop_reason: ModelStopReason::EndTurn,
            // Very verbose: 2000 output for 500 input → ratio 4.0
            usage: Some(TokenUsage {
                input_tokens: 500,
                output_tokens: 2000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
        }],
        cursor: Mutex::new(0),
    };

    let orchestrator = Orchestrator::new(
        Arc::new(provider),
        ToolRegistry::default(),
        vec![nous.clone() as Arc<dyn Middleware>],
        OrchestratorConfig {
            max_iterations: 4,
            context: None,
            context_compiler: None,
        },
    );

    let output = orchestrator.run(
        RunInput {
            run_id: "run-e2e-4".into(),
            session_id: "sess-e2e-4".into(),
            branch_id: "main".into(),
            messages: vec![ChatMessage::user("be verbose")],
            state: Default::default(),
        },
        |_| {},
    );

    assert_eq!(output.reason, RunStopReason::Completed);

    let scores = nous.scores();
    let te = scores
        .iter()
        .find(|s| s.evaluator == "token_efficiency")
        .unwrap();

    // ratio 4.0 > worst_ratio 3.0 → score should be 0.0.
    assert!(
        (te.value).abs() < f64::EPSILON,
        "token_efficiency should be 0.0 for ratio 4.0, got {}",
        te.value
    );
    assert_eq!(
        te.label,
        nous_core::ScoreLabel::Critical,
        "should be critical label"
    );
}
