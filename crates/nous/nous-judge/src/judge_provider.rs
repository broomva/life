//! LLM call wrapper for evaluation.
//!
//! Abstracts the model call so that judge evaluators don't
//! depend on a specific provider implementation.

use nous_core::NousResult;

/// Trait for making LLM calls for evaluation purposes.
///
/// Implementations should use a cost-efficient model (e.g. Haiku)
/// and include appropriate system prompts for evaluation.
pub trait JudgeProvider: Send + Sync {
    /// Send a prompt to the judge model and get a response.
    fn judge(&self, system: &str, prompt: &str) -> NousResult<String>;
}

/// Parse a judge response that contains a JSON object with score fields.
///
/// Handles responses where the model may include preamble text before the JSON,
/// or wrap the JSON in markdown code fences.
pub fn parse_judge_scores(response: &str) -> Option<serde_json::Value> {
    // Try to parse the whole response as JSON first.
    let trimmed = response.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }
    // Try to find a JSON object embedded in the response text.
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]) {
                return Some(v);
            }
        }
    }
    None
}

/// A mock judge provider for testing.
pub struct MockJudgeProvider {
    /// Fixed response to return.
    pub response: String,
}

impl JudgeProvider for MockJudgeProvider {
    fn judge(&self, _system: &str, _prompt: &str) -> NousResult<String> {
        Ok(self.response.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_provider_returns_response() {
        let provider = MockJudgeProvider {
            response: r#"{"score": 0.8, "reasoning": "good plan"}"#.into(),
        };
        let result = provider.judge("system", "evaluate this").unwrap();
        assert!(result.contains("0.8"));
    }

    #[test]
    fn parse_judge_scores_valid_json() {
        let response = r#"{"coherence": 0.9, "completeness": 0.8}"#;
        let parsed = parse_judge_scores(response).unwrap();
        assert_eq!(parsed["coherence"], 0.9);
        assert_eq!(parsed["completeness"], 0.8);
    }

    #[test]
    fn parse_judge_scores_json_in_markdown() {
        let response = r#"Here is my evaluation:
```json
{"coherence": 0.7, "completeness": 0.6}
```"#;
        let parsed = parse_judge_scores(response).unwrap();
        assert_eq!(parsed["coherence"], 0.7);
    }

    #[test]
    fn parse_judge_scores_with_preamble() {
        let response = "The plan is decent. {\"score\": 0.75}";
        let parsed = parse_judge_scores(response).unwrap();
        assert_eq!(parsed["score"], 0.75);
    }

    #[test]
    fn parse_judge_scores_no_json() {
        let response = "This is just plain text with no JSON.";
        assert!(parse_judge_scores(response).is_none());
    }

    #[test]
    fn parse_judge_scores_whitespace() {
        let response = "  \n  {\"score\": 0.5}  \n  ";
        let parsed = parse_judge_scores(response).unwrap();
        assert_eq!(parsed["score"], 0.5);
    }
}
