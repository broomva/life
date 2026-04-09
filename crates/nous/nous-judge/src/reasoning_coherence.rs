//! Reasoning coherence evaluator — did the final reasoning align with the available knowledge?

use std::sync::Arc;

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

use crate::judge_provider::{JudgeProvider, parse_judge_scores};

const SYSTEM_PROMPT: &str = "\
You are evaluating an AI agent's reasoning quality.\n\
Rate the coherence of the response from 0.0 (contradicts knowledge or incoherent) to 1.0 (fully consistent and well-reasoned).\n\
Focus on whether the response uses the available knowledge accurately, keeps a logical chain of reasoning, and avoids contradicting the supplied knowledge evidence.\n\
Respond with ONLY a JSON object:\n\
{\"score\": 0.0, \"explanation\": \"...\"}";

/// Async evaluator that scores whether the agent's reasoning is coherent with its knowledge inputs.
pub struct ReasoningCoherence {
    provider: Arc<dyn JudgeProvider>,
}

impl ReasoningCoherence {
    pub fn new(provider: Arc<dyn JudgeProvider>) -> Self {
        Self { provider }
    }
}

impl NousEvaluator for ReasoningCoherence {
    fn name(&self) -> &str {
        "reasoning_coherence"
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

        let Some(knowledge_snapshot) = knowledge_snapshot(ctx) else {
            return Ok(vec![]);
        };

        let prompt = format!(
            "Available knowledge context:\n{knowledge_snapshot}\n\nAgent response:\n{assistant_messages}"
        );

        let response = self.provider.judge(SYSTEM_PROMPT, &prompt)?;
        build_scored_response(self.name(), &response, &ctx.session_id)
    }
}

fn knowledge_snapshot(ctx: &EvalContext) -> Option<String> {
    let mut lines = Vec::new();

    if let Some(context) = ctx.metadata.get("knowledge_context")
        && !context.is_empty()
    {
        lines.push(format!("Knowledge context:\n{context}"));
    }
    if let Some(query) = &ctx.knowledge_query {
        lines.push(format!("Latest knowledge query: {query}"));
    }
    if let Some(count) = ctx.knowledge_retrieved_count {
        lines.push(format!("Retrieved notes this run: {count}"));
    }
    if let Some(top) = ctx.knowledge_top_relevance {
        lines.push(format!("Top relevance score: {top:.3}"));
    }
    if let Some(coverage) = ctx.knowledge_coverage {
        lines.push(format!("Knowledge coverage: {coverage:.3}"));
    }
    if let Some(freshness) = ctx.knowledge_freshness {
        lines.push(format!("Knowledge freshness: {freshness:.3}"));
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn build_scored_response(
    evaluator: &str,
    response: &str,
    session_id: &str,
) -> NousResult<Vec<EvalScore>> {
    if let Some(parsed) = parse_judge_scores(response) {
        if let Some(score) = parsed.get("score").and_then(serde_json::Value::as_f64) {
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
            "Based on the retrieved notes, the agent used bi-temporal validity.".into(),
        );
        ctx.metadata.insert(
            "knowledge_context".into(),
            "- temporal validity | bi-temporal notes | score: 7".into(),
        );
        ctx.knowledge_query = Some("temporal validity".into());
        ctx.knowledge_retrieved_count = Some(3);
        ctx.knowledge_top_relevance = Some(0.93);
        ctx
    }

    #[test]
    fn valid_json_response_produces_score() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"score": 0.82, "explanation": "Consistent with retrieved notes"}"#.into(),
        });
        let eval = ReasoningCoherence::new(provider);
        let scores = eval.evaluate(&ctx()).unwrap();

        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].evaluator, "reasoning_coherence");
        assert!((scores[0].value - 0.82).abs() < f64::EPSILON);
        assert_eq!(
            scores[0].explanation.as_deref(),
            Some("Consistent with retrieved notes")
        );
    }

    #[test]
    fn missing_knowledge_snapshot_returns_empty() {
        let provider = Arc::new(MockJudgeProvider {
            response: r#"{"score": 0.8}"#.into(),
        });
        let eval = ReasoningCoherence::new(provider);
        let mut ctx = EvalContext::new("test-session");
        ctx.metadata
            .insert("assistant_messages".into(), "reasoning only".into());
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn fallback_numeric_extraction_works() {
        let provider = Arc::new(MockJudgeProvider {
            response: "Overall coherence 0.74".into(),
        });
        let eval = ReasoningCoherence::new(provider);
        let scores = eval.evaluate(&ctx()).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.74).abs() < f64::EPSILON);
    }
}
