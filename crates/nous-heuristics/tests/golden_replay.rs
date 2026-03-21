//! Golden fixture replay tests.
//!
//! Loads fixture JSON files and replays them through the Nous evaluators,
//! asserting that scores fall within expected ranges.

use std::collections::HashMap;

use nous_core::{EvalContext, EvalHook};
use nous_heuristics::default_registry;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    session_id: String,
    run_id: String,
    input_tokens: u64,
    output_tokens: u64,
    tokens_remaining: u64,
    total_tokens_used: u64,
    max_iterations: u32,
    iteration: u32,
    tool_call_count: u32,
    tool_error_count: u32,
    expected_scores: HashMap<String, ScoreRange>,
}

#[derive(Deserialize)]
struct ScoreRange {
    min: f64,
    max: f64,
}

fn load_fixture(name: &str) -> Fixture {
    let path = format!("{}/fixtures/golden/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {path}: {e}"));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("failed to parse fixture {path}: {e}"))
}

fn run_fixture(fixture: &Fixture) {
    let registry = default_registry().unwrap();

    let mut ctx = EvalContext::new(&fixture.session_id);
    ctx.run_id = Some(fixture.run_id.clone());
    ctx.input_tokens = Some(fixture.input_tokens);
    ctx.output_tokens = Some(fixture.output_tokens);
    ctx.tokens_remaining = Some(fixture.tokens_remaining);
    ctx.total_tokens_used = Some(fixture.total_tokens_used);
    ctx.max_iterations = Some(fixture.max_iterations);
    ctx.iteration = Some(fixture.iteration);
    ctx.tool_call_count = Some(fixture.tool_call_count);
    ctx.tool_error_count = Some(fixture.tool_error_count);

    // Run all evaluators against the context.
    let hooks = [
        EvalHook::BeforeModelCall,
        EvalHook::AfterModelCall,
        EvalHook::PreToolCall,
        EvalHook::PostToolCall,
        EvalHook::OnRunFinished,
    ];

    let mut all_scores = HashMap::new();

    for hook in &hooks {
        for evaluator in registry.evaluators_for(*hook) {
            if let Ok(scores) = evaluator.evaluate(&ctx) {
                for score in scores {
                    all_scores.insert(score.evaluator.clone(), score.value);
                }
            }
        }
    }

    // Assert scores fall within expected ranges.
    for (evaluator_name, range) in &fixture.expected_scores {
        let score = all_scores.get(evaluator_name).unwrap_or_else(|| {
            panic!(
                "fixture '{}': expected score from evaluator '{evaluator_name}' but none produced",
                fixture.name
            )
        });
        assert!(
            *score >= range.min && *score <= range.max,
            "fixture '{}': evaluator '{evaluator_name}' score {score:.4} not in [{}, {}]",
            fixture.name,
            range.min,
            range.max
        );
    }
}

#[test]
fn golden_simple_chat() {
    let fixture = load_fixture("simple_chat");
    run_fixture(&fixture);
}

#[test]
fn golden_tool_use() {
    let fixture = load_fixture("tool_use");
    run_fixture(&fixture);
}
