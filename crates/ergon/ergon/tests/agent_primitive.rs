//! Integration tests for the Agent / TypedAgent / AgentSpec primitive.
//!
//! All tests use a `ScriptedProvider` that returns pre-canned model
//! responses — no real network. The point is to validate the
//! interpreter's behavior against every distinct outcome path:
//!
//! 1. Static `TypedAgent` happy path (single turn, valid answer).
//! 2. Multi-turn agent that uses workflow tools, then records answer.
//! 3. Schema-violation retry path (first attempt fails validation,
//!    second attempt succeeds with a corrective user message).
//! 4. Retry exhaustion → typed `SchemaViolation` error.
//! 5. Provider returns a refusal stop reason → typed `Refusal`.
//! 6. Model never calls `record_answer` → typed `AnswerNotEmitted`.
//! 7. Dynamic `AgentSpec::run` produces the same result as the
//!    equivalent `TypedAgent`.
//! 8. Agent emits an `AgentSpec` as its output (the factory pattern
//!    that proves agent-emits-agent works without recursion machinery).
//! 9. Sub-context isolation — running an agent does not pollute the
//!    parent's message history.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use ergon::{
    Agent, AgentError, AgentSpec, BufferSink, ContentBlock, ErgonError, HookRegistry, Message,
    ModelRequest, ModelResponse, Provider, RuntimeHandle, StepCtx, StopReason, StreamSink,
    ToolCall, ToolDefinition, ToolRegistry, ToolResult, TypedAgent, run_spec,
};
use ergon::{MessageRole, RECORD_ANSWER_TOOL};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Test types ──────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug, Clone)]
struct ScoreInput {
    text: String,
}

#[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug, Clone)]
struct Score {
    novelty: u8,
    specificity: u8,
    relevance: u8,
}

struct StaticScorer {
    name: &'static str,
    instructions: &'static str,
    model: &'static str,
    max_turns: u32,
    max_retries: u8,
}

impl Default for StaticScorer {
    fn default() -> Self {
        Self {
            name: "test.scorer",
            instructions: "You score the input on three axes (novelty, specificity, relevance) \
                           each 0-3. Higher is better.",
            model: "test-model",
            max_turns: 1,
            max_retries: 3,
        }
    }
}

impl TypedAgent for StaticScorer {
    type Input = ScoreInput;
    type Output = Score;
    fn name(&self) -> &str {
        self.name
    }
    fn instructions(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.instructions)
    }
    fn model(&self) -> &str {
        self.model
    }
    fn max_turns(&self) -> u32 {
        self.max_turns
    }
    fn max_retries(&self) -> u8 {
        self.max_retries
    }
}

// ── ScriptedProvider ────────────────────────────────────────────────────

/// Returns pre-canned `ModelResponse`s in order. Each call pops the
/// front of the queue. Panics if the queue is empty (means the test
/// expected fewer turns than we got).
struct ScriptedProvider {
    name: String,
    queue: Mutex<Vec<ModelResponse>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            name: "scripted".to_owned(),
            queue: Mutex::new(responses),
        }
    }

    fn remaining(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn stream(
        &self,
        _req: ModelRequest,
        _sink: Arc<dyn StreamSink>,
    ) -> Result<ModelResponse, ErgonError> {
        let mut q = self.queue.lock().unwrap();
        if q.is_empty() {
            panic!("ScriptedProvider queue exhausted — test asked for more turns than scripted");
        }
        Ok(q.remove(0))
    }
}

// ── Helpers to build canned ModelResponses ──────────────────────────────

fn record_answer_call(call_id: &str, answer: serde_json::Value) -> ModelResponse {
    ModelResponse::new(
        vec![ContentBlock::ToolUse {
            id: call_id.to_owned(),
            name: RECORD_ANSWER_TOOL.to_owned(),
            input: serde_json::json!({"answer": answer}),
        }],
        StopReason::ToolUse,
    )
}

fn end_turn_text(text: &str) -> ModelResponse {
    ModelResponse::new(
        vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        StopReason::EndTurn,
    )
}

fn refusal(text: &str) -> ModelResponse {
    ModelResponse::new(
        vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        StopReason::Refusal,
    )
}

// ── EmptyTools / NopRuntime fixtures ────────────────────────────────────

#[derive(Default)]
struct EmptyTools;

#[async_trait]
impl ToolRegistry for EmptyTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }
    async fn invoke(&self, call: ToolCall) -> Result<ToolResult, ErgonError> {
        Err(ErgonError::Tool(format!(
            "EmptyTools cannot invoke `{}`",
            call.name
        )))
    }
}

struct NopRuntime;
impl RuntimeHandle for NopRuntime {
    fn operating_mode(&self) -> aios_protocol::mode::OperatingMode {
        aios_protocol::mode::OperatingMode::Execute
    }
}

// Build a clean StepCtx for one test invocation.
fn make_ctx<'a>(
    workflow_name: &'a str,
    provider: Arc<dyn Provider>,
    sink: Arc<BufferSink>,
) -> StepCtx<'a> {
    StepCtx::new(
        ergon::SessionId::default(),
        workflow_name,
        provider,
        Arc::new(EmptyTools) as Arc<dyn ToolRegistry>,
        Arc::new(HookRegistry::default()),
        sink as Arc<dyn StreamSink>,
        Arc::new(NopRuntime) as Arc<dyn RuntimeHandle>,
        tracing::Span::current(),
    )
}

// ── 1. Static TypedAgent happy path (single turn) ──────────────────────

#[tokio::test]
async fn typed_agent_records_valid_answer_in_one_turn() {
    // With max_turns=1, the autonomous loop runs the model exactly
    // once; the model's record_answer tool_use is dispatched in that
    // turn (capturing the answer in the side channel) and the loop
    // exits with MaxTurns. The interpreter sees the captured answer
    // and returns success — the post-record EndTurn confirmation is
    // not required when max_turns budget is already spent.
    let provider = Arc::new(ScriptedProvider::new(vec![record_answer_call(
        "tu-1",
        serde_json::json!({"novelty": 2, "specificity": 3, "relevance": 3}),
    )]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = make_ctx("test-workflow", provider.clone() as Arc<dyn Provider>, sink);

    let agent = StaticScorer::default();
    let input = serde_json::to_value(ScoreInput {
        text: "demo".into(),
    })
    .unwrap();

    let answer = agent.run(&mut ctx, input).await.expect("run ok");
    let parsed: Score = serde_json::from_value(answer).expect("deserialize Score");
    assert_eq!(
        parsed,
        Score {
            novelty: 2,
            specificity: 3,
            relevance: 3
        }
    );
    assert_eq!(provider.remaining(), 0);
}

// ── 2. Multi-turn: agent uses a workflow tool, then records answer ────

#[derive(Default)]
struct StaticGreeterTool;

#[async_trait]
impl ToolRegistry for StaticGreeterTool {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition::new(
            "greet",
            "Generate a greeting",
            serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}}}),
        )]
    }
    async fn invoke(&self, call: ToolCall) -> Result<ToolResult, ErgonError> {
        let name = call
            .input
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("anon");
        Ok(ToolResult::success(
            call.id,
            serde_json::json!({"greeting": format!("hello, {name}")}),
        ))
    }
}

struct GreetThenScore;
impl TypedAgent for GreetThenScore {
    type Input = ScoreInput;
    type Output = Score;
    fn name(&self) -> &str {
        "test.greet-then-score"
    }
    fn instructions(&self) -> Cow<'_, str> {
        Cow::Borrowed("Greet then score.")
    }
    fn model(&self) -> &str {
        "test-model"
    }
    fn max_turns(&self) -> u32 {
        4
    }
}

#[tokio::test]
async fn agent_uses_workflow_tool_then_records_answer() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        // Turn 1: model calls workflow tool `greet`
        ModelResponse::new(
            vec![ContentBlock::ToolUse {
                id: "tu-1".into(),
                name: "greet".into(),
                input: serde_json::json!({"name": "Daisy"}),
            }],
            StopReason::ToolUse,
        ),
        // Turn 2: model calls record_answer with the score
        record_answer_call(
            "tu-2",
            serde_json::json!({"novelty": 1, "specificity": 2, "relevance": 3}),
        ),
        // Turn 3: model emits EndTurn after record_answer success
        end_turn_text("done"),
    ]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = StepCtx::new(
        ergon::SessionId::default(),
        "test-workflow",
        provider.clone() as Arc<dyn Provider>,
        Arc::new(StaticGreeterTool) as Arc<dyn ToolRegistry>,
        Arc::new(HookRegistry::default()),
        sink as Arc<dyn StreamSink>,
        Arc::new(NopRuntime) as Arc<dyn RuntimeHandle>,
        tracing::Span::current(),
    );

    let answer = GreetThenScore
        .run(
            &mut ctx,
            serde_json::to_value(ScoreInput { text: "x".into() }).unwrap(),
        )
        .await
        .expect("run ok");
    let score: Score = serde_json::from_value(answer).unwrap();
    assert_eq!(score.relevance, 3);
}

// ── 3. Schema-violation retry succeeds on attempt 2 ─────────────────────

#[tokio::test]
async fn schema_violation_then_retry_succeeds() {
    // Each retry attempt is a fresh `run_inference_streaming` call.
    // With max_turns=1 inside each attempt, exactly one provider
    // response is consumed per attempt.
    let provider = Arc::new(ScriptedProvider::new(vec![
        // Attempt 1: missing required `relevance` key — schema violation
        record_answer_call("tu-1", serde_json::json!({"novelty": 2, "specificity": 3})),
        // Attempt 2: corrected
        record_answer_call(
            "tu-2",
            serde_json::json!({"novelty": 2, "specificity": 3, "relevance": 1}),
        ),
    ]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = make_ctx("test-workflow", provider as Arc<dyn Provider>, sink);

    let answer = StaticScorer::default()
        .run(
            &mut ctx,
            serde_json::to_value(ScoreInput { text: "x".into() }).unwrap(),
        )
        .await
        .expect("run ok after retry");
    let s: Score = serde_json::from_value(answer).unwrap();
    assert_eq!(s.relevance, 1);
}

// ── 4. Retry exhaustion → SchemaViolation error ─────────────────────────

#[tokio::test]
async fn retry_exhaustion_surfaces_schema_violation() {
    let agent = StaticScorer {
        max_retries: 2,
        ..Default::default()
    };
    let provider = Arc::new(ScriptedProvider::new(vec![
        // Attempt 1 — invalid
        record_answer_call("tu-1", serde_json::json!({"novelty": 2})),
        // Attempt 2 — still invalid
        record_answer_call("tu-2", serde_json::json!({})),
    ]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = make_ctx("test-workflow", provider as Arc<dyn Provider>, sink);

    let err = agent
        .run(
            &mut ctx,
            serde_json::to_value(ScoreInput { text: "x".into() }).unwrap(),
        )
        .await
        .expect_err("must fail with SchemaViolation");
    assert!(
        format!("{err}").contains("schema validation"),
        "expected SchemaViolation message, got: {err}"
    );
}

// ── 5. Provider returns Refusal → typed Refusal ─────────────────────────

#[tokio::test]
async fn refusal_is_surfaced() {
    let provider = Arc::new(ScriptedProvider::new(vec![refusal("I cannot comply.")]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = make_ctx("test-workflow", provider as Arc<dyn Provider>, sink);

    let err = StaticScorer::default()
        .run(
            &mut ctx,
            serde_json::to_value(ScoreInput { text: "x".into() }).unwrap(),
        )
        .await
        .expect_err("must surface refusal");
    let msg = format!("{err}");
    assert!(msg.contains("refused"), "expected refusal text, got: {msg}");
    assert!(msg.contains("cannot comply"));
}

// ── 6. Model never emits record_answer → AnswerNotEmitted ──────────────

#[tokio::test]
async fn answer_not_emitted_surfaces_typed_error() {
    let agent = StaticScorer {
        max_turns: 2,
        ..Default::default()
    };
    let provider = Arc::new(ScriptedProvider::new(vec![end_turn_text(
        "I'd prefer not to.",
    )]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = make_ctx("test-workflow", provider as Arc<dyn Provider>, sink);

    let err = agent
        .run(
            &mut ctx,
            serde_json::to_value(ScoreInput { text: "x".into() }).unwrap(),
        )
        .await
        .expect_err("must fail with AnswerNotEmitted");
    let msg = format!("{err}");
    assert!(
        msg.contains("never emitted record_answer") || msg.contains("AnswerNotEmitted"),
        "expected AnswerNotEmitted, got: {msg}"
    );
}

// ── 7. Dynamic AgentSpec::run path produces same result as TypedAgent ──

#[tokio::test]
async fn dynamic_agentspec_produces_same_result_as_typed_agent() {
    let provider = Arc::new(ScriptedProvider::new(vec![record_answer_call(
        "tu-1",
        serde_json::json!({"novelty": 1, "specificity": 1, "relevance": 1}),
    )]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = make_ctx("test-workflow", provider as Arc<dyn Provider>, sink);

    // Construct an AgentSpec that mirrors what TypedAgent would
    // auto-derive — but at runtime, from data.
    let spec = AgentSpec::new(
        "dynamic.scorer",
        "test-model",
        "Score the input.",
        serde_json::json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"]
        }),
        serde_json::json!({
            "type": "object",
            "properties": {
                "novelty": {"type": "integer"},
                "specificity": {"type": "integer"},
                "relevance": {"type": "integer"}
            },
            "required": ["novelty", "specificity", "relevance"]
        }),
    )
    .with_max_turns(1);

    let input = serde_json::json!({"text": "x"});
    let answer = run_spec(&spec, &mut ctx, input).await.expect("run ok");
    assert_eq!(answer["novelty"], 1);
    assert_eq!(answer["specificity"], 1);
    assert_eq!(answer["relevance"], 1);
}

// ── 8. Agent that emits an AgentSpec — agent factory pattern ───────────

#[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug, Clone)]
struct DesignRequest {
    problem: String,
}

struct AgentDesigner;
impl TypedAgent for AgentDesigner {
    type Input = DesignRequest;
    type Output = AgentSpec; // ← outputs a spec for another agent
    fn name(&self) -> &str {
        "test.designer"
    }
    fn instructions(&self) -> Cow<'_, str> {
        Cow::Borrowed("Design an agent for the problem.")
    }
    fn model(&self) -> &str {
        "test-model"
    }
    fn max_turns(&self) -> u32 {
        1
    }
}

#[tokio::test]
async fn agent_can_emit_an_agent_spec_as_its_output() {
    // The designer's emitted spec — note we wrap it as the "answer".
    let designed = serde_json::json!({
        "name": "designed.specialist",
        "instructions": "You are the specialist.",
        "model": "specialist-model",
        "max_turns": 1,
        "max_retries": 1,
        "input_schema": {"type": "object"},
        "output_schema": {"type": "object", "properties": {"result": {"type": "string"}}, "required": ["result"]}
    });
    let provider = Arc::new(ScriptedProvider::new(vec![record_answer_call(
        "tu-1",
        designed.clone(),
    )]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = make_ctx("test-workflow", provider as Arc<dyn Provider>, sink);

    let answer = AgentDesigner
        .run(
            &mut ctx,
            serde_json::to_value(DesignRequest {
                problem: "subroutine X".into(),
            })
            .unwrap(),
        )
        .await
        .expect("designer ok");

    // The output should deserialize back into an AgentSpec — that's
    // the proof that "agent emits agent" composes through the typed
    // I/O contract without any special framework support.
    let produced: AgentSpec = serde_json::from_value(answer).expect("deserialize spec");
    assert_eq!(produced.name, "designed.specialist");
    assert_eq!(produced.model, "specialist-model");
}

// ── 9. Sub-context isolation ────────────────────────────────────────────

#[tokio::test]
async fn agent_run_does_not_pollute_parent_history() {
    let provider = Arc::new(ScriptedProvider::new(vec![record_answer_call(
        "tu-1",
        serde_json::json!({"novelty": 0, "specificity": 0, "relevance": 0}),
    )]));
    let sink = Arc::new(BufferSink::new());
    let mut ctx = make_ctx("test-workflow", provider as Arc<dyn Provider>, sink);

    // Seed parent history with a marker message.
    ctx.push_message(Message::user_text("PARENT-MARKER"));
    assert_eq!(ctx.history().len(), 1);

    StaticScorer::default()
        .run(
            &mut ctx,
            serde_json::to_value(ScoreInput { text: "x".into() }).unwrap(),
        )
        .await
        .expect("run ok");

    // Parent history must be exactly what it was BEFORE the agent
    // ran. The agent's user/assistant/tool messages should be confined
    // to the sub-context and discarded on drop.
    assert_eq!(
        ctx.history().len(),
        1,
        "parent history must NOT include sub-agent's messages; got {:?}",
        ctx.history()
    );
    let parent_first = &ctx.history()[0];
    assert_eq!(parent_first.role, MessageRole::User);
    assert!(
        parent_first
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text == "PARENT-MARKER"))
    );
}

// ── 10. AgentError variants render usefully ────────────────────────────

#[test]
fn agent_error_variants_have_useful_display() {
    let e1 = AgentError::AnswerNotEmitted {
        agent: "x".into(),
        max_turns: 5,
    };
    assert!(format!("{e1}").contains("never emitted"));

    let e2 = AgentError::SchemaViolation {
        agent: "x".into(),
        attempts: 3,
        last_error: "missing key".into(),
    };
    assert!(format!("{e2}").contains("schema validation"));
    assert!(format!("{e2}").contains("3 attempt"));

    let e3 = AgentError::Refusal {
        agent: "x".into(),
        message: "no".into(),
    };
    assert!(format!("{e3}").contains("refused"));

    let e4 = AgentError::InvalidSpec {
        agent: "x".into(),
        reason: "bad".into(),
    };
    assert!(format!("{e4}").contains("invalid"));
}

// ── 11. typed_schema sanitization works ─────────────────────────────────

#[test]
fn typed_schema_strips_dollar_schema_and_inlines_single_definition() {
    let schema = ergon::typed_schema::<Score>();
    let map = schema.as_object().expect("schema is object");
    assert!(
        !map.contains_key("$schema"),
        "expected sanitization to strip $schema"
    );
    assert!(
        !map.contains_key("$ref"),
        "expected sanitization to inline $ref"
    );
    assert!(
        !map.contains_key("definitions"),
        "expected sanitization to drop definitions when inlined"
    );
    assert_eq!(
        map.get("type").and_then(|v| v.as_str()),
        Some("object"),
        "schema must declare type=object after sanitization"
    );
}

// ── 12. AgentSpec roundtrips through JSON cleanly ──────────────────────

#[test]
fn agentspec_roundtrips_through_json() {
    let original = AgentSpec::new(
        "demo",
        "m1",
        "do X",
        serde_json::json!({"type": "object"}),
        serde_json::json!({"type": "object"}),
    )
    .with_max_turns(4)
    .with_max_retries(2)
    .with_allowed_tools(vec!["a".into(), "b".into()]);

    let json = serde_json::to_string(&original).expect("serialize");
    let back: AgentSpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, original.name);
    assert_eq!(back.allowed_tools, original.allowed_tools);
    assert_eq!(back.max_turns, original.max_turns);
}

// ── 13. AgentSpec::validate catches bad shapes ─────────────────────────

#[test]
fn agentspec_validate_rejects_empty_name() {
    let spec = AgentSpec::new(
        "",
        "m",
        "x",
        serde_json::json!({"type": "object"}),
        serde_json::json!({"type": "object"}),
    );
    assert!(spec.validate().is_err());
}

#[test]
fn agentspec_validate_rejects_zero_max_turns() {
    let spec = AgentSpec::new(
        "x",
        "m",
        "x",
        serde_json::json!({"type": "object"}),
        serde_json::json!({"type": "object"}),
    )
    .with_max_turns(0);
    assert!(spec.validate().is_err());
}
