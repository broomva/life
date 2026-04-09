//! Knowledge utilization evaluator — did the agent meaningfully use the retrieved knowledge?

use std::sync::Arc;

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

use crate::judge_provider::{JudgeProvider, parse_judge_scores};

const SYSTEM_PROMPT: &str = "\
An AI agent was given knowledge context before producing a response.\n\
Rate how well the agent utilized that knowledge from 0.0 (ignored it) to 1.0 (fully integrated it into the reasoning and answer).\n\
Respond with ONLY a JSON object:\n\
{\"score\": 0.0, \"explanation\": \"...\"}";

/// Async evaluator that scores whether retrieved knowledge was actually used.
pub struct KnowledgeUtilization {
    provider: Arc<dyn JudgeProvider>,
}

impl KnowledgeUtilization {
    pub fn new(provider: Arc<dyn JudgeProvider>) -> Self {
        Self { provider }
    }
}

impl NousEvaluator for KnowledgeUtilization {
    fn name(&self) -> &str {
        "knowledge_utilization"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Async
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let assistant_messages = match ctx.metadata.get("assistant_messages") {
            Some(messages) if !messages.is_empty() => messages,
            _ => return Ok(vec![]),
        };

        let Some(knowledge_snapshot) = utilization_snapshot(ctx) else {
            return Ok(vec![]);
        };

        let prompt = format!(
            "Knowledge evidence:\n{knowledge_snapshot}\n\nAgent response:\n{assistant_messages}"
        );
        let response = self.provider.judge(SYSTEM_PROMPT, &prompt)?;
        scored_response(self.name(), &response, &ctx.session_id)
    }
}

fn utilization_snapshot(ctx: &EvalContext) -> Option<String> {
    let mut lines = Vec::new();

    if let Some(context) = ctx.metadata.get("knowledge_context")
        && !context.is_empty()
    {
        lines.push(format!("Retrieved knowledge context:\n{context}"));
    }
    if let Some(query) = &ctx.knowledge_query {
        lines.push(format!("Most recent query: {query}"));
    }
    if let Some(count) = ctx.knowledge_retrieved_count {
        lines.push(format!("Retrieved notes this run: {count}"));
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn scored_response(
    evaluator: &str,
    response: &str,
    session_id: &str,
) -> NousResult<Vec<EvalScore>> {
    if let Some(parsed) = parse_judge_scores(response)
        && let Some(score) = parsed.get("score").and_then(serde_json::Value::as_f64)
    {
        let mut eval = EvalScore::new(
            evaluator,
            score.clamp(0.0, 1.0),
            EvalLayer::Reasoning,
            EvalTiming::Async,
            session_id,
        )?;
        if let Some(explanation) = parsed.get("explanation").and_then(|v| v.as_str()) {
            eval = eval.with_explanation(explanation.to_owned());
        }
        return Ok(vec![eval]);
    }

    for token in response.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        if let Ok(score) = cleaned.parse::<f64>()
            && (0.0..=1.0).contains(&score)
        {
            return Ok(vec![EvalScore::new(
                evaluator,
                score,
                EvalLayer::Reasoning,
                EvalTiming::Async,
                session_id,
            )?]);
        }
    }

    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge_provider::MockJudgeProvider;

    fn ctx() -> EvalContext {
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata.insert(
            "assistant_messages".into(),
            "The answer directly cites the retrieved temporal validity notes.".into(),
        );
        ctx.metadata.insert(
            "knowledge_context".into(),
            "- temporal validity | valid_from / valid_to | score: 7".into(),
        );
        ctx.knowledge_query = Some("temporal validity".into());
        ctx.knowledge_retrieved_count = Some(2);
        ctx
    }

    #[test]
    fn valid_json_response_produces_score() {
        let provider = Arc::new(MockJudgeProvider {
            response:
                r#"{"score": 0.91, "explanation": "Response clearly uses retrieved context"}"#
                    .into(),
        });
        let eval = KnowledgeUtilization::new(provider);
        let scores = eval.evaluate(&ctx()).unwrap();

        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].evaluator, "knowledge_utilization");
        assert!((scores[0].value - 0.91).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_snapshot_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"score": 0.7}"#.into(),
        });
        let eval = KnowledgeUtilization::new(provider);
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata
            .insert("assistant_messages".into(), "answer only".into());
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn fallback_numeric_extraction_works() {
        let provider = Arc::new(MockJudgeProvider {
            response: "Utilization score: 0.66".into(),
        });
        let eval = KnowledgeUtilization::new(provider);
        let scores = eval.evaluate(&ctx()).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.66).abs() < f64::EPSILON);
    }
}
