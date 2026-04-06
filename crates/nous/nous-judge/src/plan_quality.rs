//! Plan quality evaluator — LLM-as-judge for reasoning coherence.
//!
//! Assesses whether the agent's plan is logically sound,
//! complete, and well-structured by calling a judge LLM.

use std::sync::Arc;

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

use crate::judge_provider::{JudgeProvider, parse_judge_scores};

/// System prompt for plan quality assessment.
const SYSTEM_PROMPT: &str = "\
You are an expert evaluator assessing the quality of an AI agent's reasoning. \
Score each dimension from 0.0 (terrible) to 1.0 (excellent). \
Respond with ONLY a JSON object, no other text:\n\
{\"coherence\": 0.0, \"completeness\": 0.0, \"logical_soundness\": 0.0}";

/// Evaluates the quality of the agent's planning/reasoning.
///
/// Uses an LLM call to assess coherence, completeness, and logical soundness.
/// Runs asynchronously after a run completes.
pub struct PlanQuality {
    provider: Arc<dyn JudgeProvider>,
}

impl PlanQuality {
    /// Create a new plan quality evaluator with the given judge provider.
    pub fn new(provider: Arc<dyn JudgeProvider>) -> Self {
        Self { provider }
    }
}

impl NousEvaluator for PlanQuality {
    fn name(&self) -> &str {
        "plan_quality"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Async
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        // Look for assistant messages in metadata.
        let assistant_messages = match ctx.metadata.get("assistant_messages") {
            Some(msgs) if !msgs.is_empty() => msgs,
            _ => return Ok(vec![]),
        };

        let prompt = format!(
            "Evaluate the quality of the following AI agent reasoning:\n\n{}",
            assistant_messages
        );

        let response = self.provider.judge(SYSTEM_PROMPT, &prompt)?;

        let scores = if let Some(parsed) = parse_judge_scores(&response) {
            let mut result = Vec::new();

            let dimensions = [
                ("plan_quality.coherence", "coherence"),
                ("plan_quality.completeness", "completeness"),
                ("plan_quality.soundness", "logical_soundness"),
            ];

            for (eval_name, json_key) in &dimensions {
                if let Some(value) = parsed.get(json_key).and_then(serde_json::Value::as_f64) {
                    let clamped = value.clamp(0.0, 1.0);
                    let score = EvalScore::new(
                        *eval_name,
                        clamped,
                        EvalLayer::Reasoning,
                        EvalTiming::Async,
                        &ctx.session_id,
                    )?;
                    result.push(score);
                }
            }

            result
        } else {
            // Try to extract any numeric value as a fallback.
            extract_fallback_score(&response, &ctx.session_id)?
        };

        Ok(scores)
    }
}

/// Attempt to extract a single numeric score from a free-form response.
fn extract_fallback_score(response: &str, session_id: &str) -> NousResult<Vec<EvalScore>> {
    // Look for a floating-point number in the response.
    for word in response.split_whitespace() {
        let cleaned = word.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        if let Ok(value) = cleaned.parse::<f64>()
            && (0.0..=1.0).contains(&value)
        {
            let score = EvalScore::new(
                "plan_quality",
                value,
                EvalLayer::Reasoning,
                EvalTiming::Async,
                session_id,
            )?;
            return Ok(vec![score]);
        }
    }
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge_provider::MockJudgeProvider;

    fn make_ctx_with_messages(messages: &str) -> EvalContext {
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata
            .insert("assistant_messages".into(), messages.into());
        ctx
    }

    #[test]
    fn valid_json_response_produces_three_scores() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"coherence": 0.9, "completeness": 0.8, "logical_soundness": 0.7}"#.into(),
        });
        let eval = PlanQuality::new(provider);
        let ctx = make_ctx_with_messages("Let me think step by step...");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 3);
        assert_eq!(scores[0].evaluator, "plan_quality.coherence");
        assert!((scores[0].value - 0.9).abs() < f64::EPSILON);
        assert_eq!(scores[1].evaluator, "plan_quality.completeness");
        assert!((scores[1].value - 0.8).abs() < f64::EPSILON);
        assert_eq!(scores[2].evaluator, "plan_quality.soundness");
        assert!((scores[2].value - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn malformed_response_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: "I cannot evaluate this properly, sorry.".into(),
        });
        let eval = PlanQuality::new(provider);
        let ctx = make_ctx_with_messages("Some reasoning here");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn missing_metadata_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"coherence": 0.9}"#.into(),
        });
        let eval = PlanQuality::new(provider);
        let ctx = EvalContext::new("test-session");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn empty_messages_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"coherence": 0.9}"#.into(),
        });
        let eval = PlanQuality::new(provider);
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata
            .insert("assistant_messages".into(), String::new());
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn json_in_markdown_extracted_correctly() {
        let provider = Arc::new(MockJudgeProvider {
            response: "Here is my assessment:\n```json\n{\"coherence\": 0.85, \"completeness\": 0.75, \"logical_soundness\": 0.65}\n```".into(),
        });
        let eval = PlanQuality::new(provider);
        let ctx = make_ctx_with_messages("Agent reasoning text");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 3);
        assert!((scores[0].value - 0.85).abs() < f64::EPSILON);
        assert!((scores[1].value - 0.75).abs() < f64::EPSILON);
        assert!((scores[2].value - 0.65).abs() < f64::EPSILON);
    }

    #[test]
    fn partial_json_produces_partial_scores() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"coherence": 0.9}"#.into(),
        });
        let eval = PlanQuality::new(provider);
        let ctx = make_ctx_with_messages("Some reasoning");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].evaluator, "plan_quality.coherence");
    }

    #[test]
    fn fallback_numeric_extraction() {
        let provider = Arc::new(MockJudgeProvider {
            response: "The overall quality is 0.75 out of 1.0".into(),
        });
        let eval = PlanQuality::new(provider);
        let ctx = make_ctx_with_messages("Some reasoning");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].evaluator, "plan_quality");
        assert!((scores[0].value - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn scores_are_clamped_to_valid_range() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"coherence": 1.5, "completeness": -0.3, "logical_soundness": 0.5}"#
                .into(),
        });
        let eval = PlanQuality::new(provider);
        let ctx = make_ctx_with_messages("Reasoning");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 3);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON); // clamped from 1.5
        assert!((scores[1].value - 0.0).abs() < f64::EPSILON); // clamped from -0.3
        assert!((scores[2].value - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluator_metadata() {
        let provider = Arc::new(MockJudgeProvider {
            response: String::new(),
        });
        let eval = PlanQuality::new(provider);
        assert_eq!(eval.name(), "plan_quality");
        assert_eq!(eval.layer(), EvalLayer::Reasoning);
        assert_eq!(eval.timing(), EvalTiming::Async);
    }
}
