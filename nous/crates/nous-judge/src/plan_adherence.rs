//! Plan adherence evaluator — did the agent follow its stated plan?
//!
//! Uses an LLM-as-judge call to compare the agent's stated intentions
//! with its actual actions (tool calls, outputs).

use std::sync::Arc;

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

use crate::judge_provider::{JudgeProvider, parse_judge_scores};

/// System prompt sent to the judge model for plan adherence evaluation.
const SYSTEM_PROMPT: &str = "\
You are an expert evaluator assessing whether an AI agent followed its stated plan.\n\
Compare the agent's stated intentions with its actual actions.\n\
If no explicit plan was stated, score 1.0 (benefit of the doubt).\n\
Respond with ONLY a JSON object:\n\
{\"adherence\": 0.0, \"explanation\": \"...\"}";

/// Evaluates whether the agent's execution matched its stated plan.
///
/// Compares the agent's assistant messages (which may contain a stated plan)
/// against the tool calls summary (what actually happened) using an LLM judge.
pub struct PlanAdherence {
    provider: Arc<dyn JudgeProvider>,
}

impl PlanAdherence {
    /// Create a new `PlanAdherence` evaluator with the given judge provider.
    pub fn new(provider: Arc<dyn JudgeProvider>) -> Self {
        Self { provider }
    }
}

impl NousEvaluator for PlanAdherence {
    fn name(&self) -> &str {
        "plan_adherence"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Async
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        // We need assistant_messages to evaluate plan adherence.
        let assistant_messages = match ctx.metadata.get("assistant_messages") {
            Some(msgs) if !msgs.is_empty() => msgs,
            _ => return Ok(vec![]),
        };

        // tool_calls_summary is optional — the agent may not have made tool calls.
        let tool_calls_summary = ctx
            .metadata
            .get("tool_calls_summary")
            .cloned()
            .unwrap_or_default();

        let prompt = format!(
            "## Agent's messages (may contain stated plan):\n{assistant_messages}\n\n\
             ## Actual tool calls executed:\n{tool_calls_summary}"
        );

        let response = self.provider.judge(SYSTEM_PROMPT, &prompt)?;

        let Some(parsed) = parse_judge_scores(&response) else {
            return Ok(vec![]);
        };

        let adherence = match parsed.get("adherence").and_then(serde_json::Value::as_f64) {
            Some(v) if (0.0..=1.0).contains(&v) => v,
            _ => return Ok(vec![]),
        };

        let explanation = parsed
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("No explanation provided");

        let score = EvalScore::new(
            self.name(),
            adherence,
            self.layer(),
            self.timing(),
            &ctx.session_id,
        )?
        .with_explanation(explanation);

        Ok(vec![score])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge_provider::MockJudgeProvider;

    fn ctx_with_metadata(
        assistant_messages: Option<&str>,
        tool_calls_summary: Option<&str>,
    ) -> EvalContext {
        let mut ctx = EvalContext::new("test-session");
        if let Some(msgs) = assistant_messages {
            ctx.metadata
                .insert("assistant_messages".into(), msgs.into());
        }
        if let Some(summary) = tool_calls_summary {
            ctx.metadata
                .insert("tool_calls_summary".into(), summary.into());
        }
        ctx
    }

    #[test]
    fn plan_stated_and_followed() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"adherence": 0.95, "explanation": "Agent followed plan closely"}"#.into(),
        });
        let eval = PlanAdherence::new(provider);
        let ctx = ctx_with_metadata(
            Some("I will first read the file, then edit it."),
            Some("read_file(path='src/main.rs'), edit_file(path='src/main.rs')"),
        );
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].evaluator, "plan_adherence");
        assert!((scores[0].value - 0.95).abs() < f64::EPSILON);
        assert_eq!(
            scores[0].explanation.as_deref(),
            Some("Agent followed plan closely")
        );
        assert_eq!(scores[0].layer, EvalLayer::Reasoning);
        assert_eq!(scores[0].timing, EvalTiming::Async);
    }

    #[test]
    fn no_plan_stated_benefits_of_doubt() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"adherence": 1.0, "explanation": "No explicit plan was stated"}"#.into(),
        });
        let eval = PlanAdherence::new(provider);
        let ctx = ctx_with_metadata(
            Some("Here is the answer to your question."),
            Some("search(query='rust async')"),
        );
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_metadata_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"adherence": 0.5, "explanation": "test"}"#.into(),
        });
        let eval = PlanAdherence::new(provider);

        // No metadata at all.
        let ctx = EvalContext::new("test-session");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());

        // Empty assistant_messages.
        let ctx = ctx_with_metadata(Some(""), None);
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn malformed_judge_response_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: "I cannot evaluate this properly, sorry.".into(),
        });
        let eval = PlanAdherence::new(provider);
        let ctx = ctx_with_metadata(
            Some("I will fix the bug."),
            Some("edit_file(path='src/lib.rs')"),
        );
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn adherence_out_of_range_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"adherence": 1.5, "explanation": "invalid score"}"#.into(),
        });
        let eval = PlanAdherence::new(provider);
        let ctx = ctx_with_metadata(Some("I will do something."), Some("tool_call()"));
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn json_wrapped_in_markdown() {
        let provider = Arc::new(MockJudgeProvider {
            response: "Here is my evaluation:\n```json\n{\"adherence\": 0.7, \"explanation\": \"Partially followed\"}\n```".into(),
        });
        let eval = PlanAdherence::new(provider);
        let ctx = ctx_with_metadata(
            Some("I will read, then write."),
            Some("read_file(), write_file()"),
        );
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.7).abs() < f64::EPSILON);
        assert_eq!(scores[0].explanation.as_deref(), Some("Partially followed"));
    }

    #[test]
    fn missing_explanation_uses_default() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"adherence": 0.8}"#.into(),
        });
        let eval = PlanAdherence::new(provider);
        let ctx = ctx_with_metadata(Some("Plan: do the thing."), Some("did_the_thing()"));
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert_eq!(
            scores[0].explanation.as_deref(),
            Some("No explanation provided")
        );
    }
}
