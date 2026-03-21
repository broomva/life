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
}
