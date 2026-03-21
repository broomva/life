//! Task completion evaluator — did the agent achieve its goal?
//!
//! Uses an LLM-as-judge to assess whether the agent's final answer
//! successfully addresses the stated objective.

use std::sync::Arc;

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

use crate::judge_provider::{JudgeProvider, parse_judge_scores};

/// System prompt for task completion assessment.
const SYSTEM_PROMPT: &str = "\
You are an expert evaluator assessing whether an AI agent successfully completed its task. \
Given the objective and the agent's final answer, score task completion from 0.0 (not completed at all) to 1.0 (perfectly completed). \
Respond with ONLY a JSON object, no other text:\n\
{\"completion\": 0.0}";

/// Evaluates whether the agent successfully completed its assigned task.
pub struct TaskCompletion {
    provider: Arc<dyn JudgeProvider>,
}

impl TaskCompletion {
    /// Create a new task completion evaluator with the given judge provider.
    pub fn new(provider: Arc<dyn JudgeProvider>) -> Self {
        Self { provider }
    }
}

impl NousEvaluator for TaskCompletion {
    fn name(&self) -> &str {
        "task_completion"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Async
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        // Both objective and final_answer must be present.
        let objective = match ctx.metadata.get("objective") {
            Some(obj) if !obj.is_empty() => obj,
            _ => return Ok(vec![]),
        };
        let final_answer = match ctx.metadata.get("final_answer") {
            Some(ans) if !ans.is_empty() => ans,
            _ => return Ok(vec![]),
        };

        let prompt = format!(
            "Objective:\n{}\n\nAgent's final answer:\n{}",
            objective, final_answer
        );

        let response = self.provider.judge(SYSTEM_PROMPT, &prompt)?;

        let scores = if let Some(parsed) = parse_judge_scores(&response) {
            if let Some(value) = parsed.get("completion").and_then(serde_json::Value::as_f64) {
                let clamped = value.clamp(0.0, 1.0);
                let score = EvalScore::new(
                    "task_completion",
                    clamped,
                    EvalLayer::Reasoning,
                    EvalTiming::Async,
                    &ctx.session_id,
                )?;
                vec![score]
            } else {
                // JSON present but no "completion" key — try any numeric value.
                extract_first_numeric_value(&parsed, &ctx.session_id)?
            }
        } else {
            // Try to find a numeric value in the raw text.
            extract_fallback_score(&response, &ctx.session_id)?
        };

        Ok(scores)
    }
}

/// Extract the first numeric value from a JSON object.
fn extract_first_numeric_value(
    value: &serde_json::Value,
    session_id: &str,
) -> NousResult<Vec<EvalScore>> {
    if let Some(obj) = value.as_object() {
        for (_key, v) in obj {
            if let Some(num) = v.as_f64() {
                let clamped = num.clamp(0.0, 1.0);
                let score = EvalScore::new(
                    "task_completion",
                    clamped,
                    EvalLayer::Reasoning,
                    EvalTiming::Async,
                    session_id,
                )?;
                return Ok(vec![score]);
            }
        }
    }
    Ok(vec![])
}

/// Attempt to extract a single numeric score from a free-form response.
fn extract_fallback_score(response: &str, session_id: &str) -> NousResult<Vec<EvalScore>> {
    for word in response.split_whitespace() {
        let cleaned = word.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        if let Ok(value) = cleaned.parse::<f64>() {
            if (0.0..=1.0).contains(&value) {
                let score = EvalScore::new(
                    "task_completion",
                    value,
                    EvalLayer::Reasoning,
                    EvalTiming::Async,
                    session_id,
                )?;
                return Ok(vec![score]);
            }
        }
    }
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge_provider::MockJudgeProvider;

    fn make_ctx_with_task(objective: &str, final_answer: &str) -> EvalContext {
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata.insert("objective".into(), objective.into());
        ctx.metadata
            .insert("final_answer".into(), final_answer.into());
        ctx
    }

    #[test]
    fn valid_json_response_produces_score() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"completion": 0.85}"#.into(),
        });
        let eval = TaskCompletion::new(provider);
        let ctx = make_ctx_with_task("Write a hello world program", "print('Hello, World!')");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].evaluator, "task_completion");
        assert!((scores[0].value - 0.85).abs() < f64::EPSILON);
        assert_eq!(scores[0].layer, EvalLayer::Reasoning);
        assert_eq!(scores[0].timing, EvalTiming::Async);
    }

    #[test]
    fn malformed_response_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: "I'm not sure how to evaluate this.".into(),
        });
        let eval = TaskCompletion::new(provider);
        let ctx = make_ctx_with_task("Do something", "I did it");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn missing_objective_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"completion": 0.9}"#.into(),
        });
        let eval = TaskCompletion::new(provider);
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata
            .insert("final_answer".into(), "some answer".into());
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn missing_final_answer_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"completion": 0.9}"#.into(),
        });
        let eval = TaskCompletion::new(provider);
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata
            .insert("objective".into(), "some objective".into());
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn empty_objective_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"completion": 0.9}"#.into(),
        });
        let eval = TaskCompletion::new(provider);
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata.insert("objective".into(), String::new());
        ctx.metadata
            .insert("final_answer".into(), "some answer".into());
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn json_in_markdown_extracted_correctly() {
        let provider = Arc::new(MockJudgeProvider {
            response: "My assessment:\n```\n{\"completion\": 0.92}\n```".into(),
        });
        let eval = TaskCompletion::new(provider);
        let ctx = make_ctx_with_task("Build a REST API", "Here is the API implementation...");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.92).abs() < f64::EPSILON);
    }

    #[test]
    fn fallback_numeric_extraction() {
        let provider = Arc::new(MockJudgeProvider {
            response: "Task completion: 0.8 — mostly done.".into(),
        });
        let eval = TaskCompletion::new(provider);
        let ctx = make_ctx_with_task("Write tests", "Added 5 tests");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn score_clamped_to_valid_range() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"completion": 1.5}"#.into(),
        });
        let eval = TaskCompletion::new(provider);
        let ctx = make_ctx_with_task("Objective", "Answer");
        let scores = eval.evaluate(&ctx).unwrap();

        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn json_with_alternative_key_name() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"score": 0.6}"#.into(),
        });
        let eval = TaskCompletion::new(provider);
        let ctx = make_ctx_with_task("Objective", "Answer");
        let scores = eval.evaluate(&ctx).unwrap();

        // Falls back to extracting first numeric value from JSON object.
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluator_metadata() {
        let provider = Arc::new(MockJudgeProvider {
            response: String::new(),
        });
        let eval = TaskCompletion::new(provider);
        assert_eq!(eval.name(), "task_completion");
        assert_eq!(eval.layer(), EvalLayer::Reasoning);
        assert_eq!(eval.timing(), EvalTiming::Async);
    }
}
