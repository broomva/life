//! Integration tests for the `BookkeepingJudge` workflow.
//!
//! All tests here use a `ScriptedProvider` that returns canned
//! `record_answer` tool_use responses — no network, no real model.
//! The shape we're verifying is the harness wiring + aggregation
//! math; per-axis prompting fidelity is validated by the authored
//! agents' own offline tests + the live-Anthropic parity smoke
//! (`tests/parity_smoke.rs`, `#[ignore]`).
//!
//! ## Why a scripted provider works
//!
//! Each axis agent's `max_turns = 1` (per the authored .md frontmatter)
//! so we know exactly how many model calls happen per item:
//! `3 axes × N items` model calls per workflow run. The scripted
//! provider returns a `record_answer` tool_use per call in the
//! authored output schema's shape.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ergon::{
    BufferSink, ContentBlock, ErgonError, HookRegistry, ModelRequest, ModelResponse, Provider,
    RECORD_ANSWER_TOOL, RuntimeHandle, SessionId, StepCtx, StopReason, StreamSink, ToolCall,
    ToolDefinition, ToolRegistry, ToolResult, Workflow,
};

use bookkeeping_judge::{BookkeepingJudge, JudgeInput};

// ── Test infrastructure ────────────────────────────────────────────────

/// Returns pre-canned `record_answer` tool_use responses round-robin
/// across the three axes per item: novelty[1], specificity[1],
/// relevance[1], novelty[2], specificity[2], relevance[2], …
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
            panic!(
                "ScriptedProvider queue exhausted — workflow asked for \
                 more turns than scripted"
            );
        }
        Ok(q.remove(0))
    }
}

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

fn make_ctx<'a>(workflow_name: &'a str, provider: Arc<dyn Provider>) -> StepCtx<'a> {
    StepCtx::new(
        SessionId::default(),
        workflow_name,
        provider,
        Arc::new(EmptyTools) as Arc<dyn ToolRegistry>,
        Arc::new(HookRegistry::default()),
        Arc::new(BufferSink::new()) as Arc<dyn StreamSink>,
        Arc::new(NopRuntime) as Arc<dyn RuntimeHandle>,
        tracing::Span::current(),
    )
}

/// Synthesize a `record_answer` tool_use response. The framework's
/// run_spec interpreter wraps the typed answer in `{"answer": …}`.
fn record_answer(call_id: &str, answer: serde_json::Value) -> ModelResponse {
    ModelResponse::new(
        vec![ContentBlock::ToolUse {
            id: call_id.to_owned(),
            name: RECORD_ANSWER_TOOL.to_owned(),
            input: serde_json::json!({"answer": answer}),
        }],
        StopReason::ToolUse,
    )
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn agents_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("agents")
        .canonicalize()
        .expect("workspace agents/ dir must exist (BRO-1010 + BRO-1015 ship the three bookkeeping-* agents)")
}

// ── Tests ──────────────────────────────────────────────────────────────

/// Verifies the workflow loads the agents, parses two items, dispatches
/// three axes per item, aggregates into a `JudgeOutput`, and applies
/// the locked bellows-shipped pass/blog_candidate thresholds.
#[tokio::test]
async fn judges_two_items_with_scripted_responses() {
    let body = "# Sample\n\nSource URL: https://example.com\nSource type: x-thread\n\n## Item 1\n\nDetailed claim about ergon harness and authored agents.\n\n## Item 2\n\nVague hand-wave about AI.\n";
    let extract_path = std::env::temp_dir().join("bookkeeping-judge-test-extract.md");
    tokio::fs::write(&extract_path, body)
        .await
        .expect("write extract fixture");

    // Item 1 scores: novelty=3, specificity=3, relevance=2 → total 8 (pass + blog)
    // Item 2 scores: novelty=0, specificity=0, relevance=0 → total 0 (fail)
    let provider = Arc::new(ScriptedProvider::new(vec![
        // Item 1
        record_answer(
            "n1",
            serde_json::json!({
                "score": 3,
                "closest_existing_slug": "",
                "reasoning": "Introduces a new concept — ergon harness — not previously in the graph.",
                "anti_pattern_warnings": []
            }),
        ),
        record_answer(
            "s1",
            serde_json::json!({
                "score": 3,
                "concrete_evidence": ["ergon harness", "authored agents"],
                "reasoning": "Names a specific primitive and a concrete architectural pattern.",
                "anti_pattern_warnings": []
            }),
        ),
        record_answer(
            "r1",
            serde_json::json!({
                "score": 2,
                "connected_projects": [],
                "addresses_open_question": "",
                "reasoning": "Direct connection to active Life Agent OS work.",
                "anti_pattern_warnings": []
            }),
        ),
        // Item 2
        record_answer(
            "n2",
            serde_json::json!({
                "score": 0,
                "closest_existing_slug": "concept/ai-vague",
                "reasoning": "Pure generality; no novel claim.",
                "anti_pattern_warnings": []
            }),
        ),
        record_answer(
            "s2",
            serde_json::json!({
                "score": 0,
                "concrete_evidence": [],
                "reasoning": "No concrete entity, number, or mechanism named.",
                "anti_pattern_warnings": []
            }),
        ),
        record_answer(
            "r2",
            serde_json::json!({
                "score": 0,
                "connected_projects": [],
                "addresses_open_question": "",
                "reasoning": "No discernible connection to active projects.",
                "anti_pattern_warnings": []
            }),
        ),
    ]));

    let judge = BookkeepingJudge::new(agents_dir()).expect("load agents");
    let mut ctx = make_ctx(
        "test.bookkeeping-judge",
        provider.clone() as Arc<dyn Provider>,
    );

    let input = JudgeInput {
        extract_path: extract_path.to_string_lossy().to_string(),
        max_items: Some(10),
    };

    let output = judge.execute(&mut ctx, input).await.expect("execute ok");

    assert_eq!(output.items.len(), 2);
    assert_eq!(output.items[0].item_number, 1);
    assert_eq!(output.items[0].novelty, 3);
    assert_eq!(output.items[0].specificity, 3);
    assert_eq!(output.items[0].relevance, 2);
    assert_eq!(output.items[0].total, 8);
    assert!(output.items[0].pass);
    assert!(output.items[0].blog_candidate);

    assert_eq!(output.items[1].item_number, 2);
    assert_eq!(output.items[1].total, 0);
    assert!(!output.items[1].pass);
    assert!(!output.items[1].blog_candidate);

    assert_eq!(output.provider, "ergon-authored-agents");
    assert!(!output.judged_at.is_empty());
    assert_eq!(provider.remaining(), 0);
}

/// Verifies empty extracts produce empty output without panicking.
#[tokio::test]
async fn empty_extract_produces_empty_output() {
    let extract_path = std::env::temp_dir().join("bookkeeping-judge-empty.md");
    tokio::fs::write(&extract_path, "# Title\n\nno items here\n")
        .await
        .expect("write");

    let provider = Arc::new(ScriptedProvider::new(vec![]));
    let judge = BookkeepingJudge::new(agents_dir()).expect("load agents");
    let mut ctx = make_ctx("test.bookkeeping-judge", provider as Arc<dyn Provider>);

    let input = JudgeInput {
        extract_path: extract_path.to_string_lossy().to_string(),
        max_items: None,
    };

    let output = judge.execute(&mut ctx, input).await.expect("ok");
    assert!(output.items.is_empty());
    assert_eq!(output.provider, "ergon-authored-agents");
}

/// Verifies the sample fixture file under `tests/fixtures/` is
/// machine-parsable and the workflow loads + walks it cleanly.
#[tokio::test]
async fn parses_sample_fixture() {
    let extract = fixtures_dir().join("sample-raw.md");
    assert!(extract.exists(), "fixture file must ship in the crate");

    // Provide three identical mid-score axes per item (3 items → 9 responses).
    let mut responses = Vec::new();
    for _ in 0..3 {
        responses.push(record_answer(
            "n",
            serde_json::json!({
                "score": 2,
                "closest_existing_slug": "",
                "reasoning": "Adequate.",
                "anti_pattern_warnings": []
            }),
        ));
        responses.push(record_answer(
            "s",
            serde_json::json!({
                "score": 2,
                "concrete_evidence": ["sample evidence"],
                "reasoning": "Adequate.",
                "anti_pattern_warnings": []
            }),
        ));
        responses.push(record_answer(
            "r",
            serde_json::json!({
                "score": 1,
                "connected_projects": [],
                "addresses_open_question": "",
                "reasoning": "Adequate.",
                "anti_pattern_warnings": []
            }),
        ));
    }
    let provider = Arc::new(ScriptedProvider::new(responses));
    let judge = BookkeepingJudge::new(agents_dir()).expect("load agents");
    let mut ctx = make_ctx("test.bookkeeping-judge", provider as Arc<dyn Provider>);

    let input = JudgeInput {
        extract_path: extract.to_string_lossy().to_string(),
        max_items: Some(10),
    };

    let output = judge.execute(&mut ctx, input).await.expect("ok");
    assert_eq!(output.items.len(), 3);
    for (i, item) in output.items.iter().enumerate() {
        assert_eq!(item.item_number, (i + 1) as u32);
        assert_eq!(item.total, 5);
        assert!(item.pass);
        assert!(!item.blog_candidate);
    }
}

/// Verifies max_items truncation works at the workflow boundary
/// (matches the bellows reference hard-cap behavior).
#[tokio::test]
async fn max_items_caps_judgments() {
    let body = "## Item 1\n\nbody\n\n## Item 2\n\nbody\n\n## Item 3\n\nbody\n";
    let extract_path = std::env::temp_dir().join("bookkeeping-judge-cap.md");
    tokio::fs::write(&extract_path, body).await.expect("write");

    // Only 1 item × 3 axes = 3 responses needed even though the file has 3 items.
    let provider = Arc::new(ScriptedProvider::new(vec![
        record_answer(
            "n",
            serde_json::json!({
                "score": 1, "closest_existing_slug": "", "reasoning": "ok", "anti_pattern_warnings": []
            }),
        ),
        record_answer(
            "s",
            serde_json::json!({
                "score": 1, "concrete_evidence": ["x"], "reasoning": "ok", "anti_pattern_warnings": []
            }),
        ),
        record_answer(
            "r",
            serde_json::json!({
                "score": 1, "connected_projects": [], "addresses_open_question": "", "reasoning": "ok", "anti_pattern_warnings": []
            }),
        ),
    ]));
    let judge = BookkeepingJudge::new(agents_dir()).expect("load agents");
    let mut ctx = make_ctx(
        "test.bookkeeping-judge",
        provider.clone() as Arc<dyn Provider>,
    );

    let input = JudgeInput {
        extract_path: extract_path.to_string_lossy().to_string(),
        max_items: Some(1),
    };

    let output = judge.execute(&mut ctx, input).await.expect("ok");
    assert_eq!(output.items.len(), 1);
    assert_eq!(output.items[0].item_number, 1);
    assert_eq!(provider.remaining(), 0);
}
