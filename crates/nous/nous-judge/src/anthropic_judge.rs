//! Anthropic API-backed judge provider for real LLM-as-judge evaluation.
//!
//! Uses the Anthropic Messages API with a cost-efficient model (Haiku by default)
//! to evaluate agent outputs. Requires `ANTHROPIC_API_KEY` environment variable.

use crate::judge_provider::JudgeProvider;
use nous_core::{NousError, NousResult};

/// Judge provider that calls the Anthropic Messages API.
///
/// Self-contained HTTP client using `reqwest::blocking::Client` since
/// [`JudgeProvider::judge`] is synchronous.
pub struct AnthropicJudgeProvider {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
}

impl AnthropicJudgeProvider {
    /// Create from environment. Reads `ANTHROPIC_API_KEY`.
    pub fn from_env() -> NousResult<Self> {
        let api_key =
            std::env::var("ANTHROPIC_API_KEY").map_err(|_| NousError::EvaluatorFailed {
                name: "anthropic_judge".into(),
                message: "ANTHROPIC_API_KEY not set".into(),
            })?;
        Ok(Self {
            client: reqwest::blocking::Client::new(),
            api_key,
            model: "claude-haiku-4-5-20251001".into(),
        })
    }

    /// Create with explicit config.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

impl JudgeProvider for AnthropicJudgeProvider {
    fn judge(&self, system: &str, prompt: &str) -> NousResult<String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 256,
            "system": system,
            "messages": [{"role": "user", "content": prompt}]
        });

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| NousError::EvaluatorFailed {
                name: "anthropic_judge".into(),
                message: format!("HTTP request failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            return Err(NousError::EvaluatorFailed {
                name: "anthropic_judge".into(),
                message: format!("API error {status}: {text}"),
            });
        }

        let json: serde_json::Value = response.json().map_err(|e| NousError::EvaluatorFailed {
            name: "anthropic_judge".into(),
            message: format!("failed to parse response: {e}"),
        })?;

        // Extract text from content[0].text
        json.get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|block| block.get("text"))
            .and_then(|t| t.as_str())
            .map(str::to_owned)
            .ok_or_else(|| NousError::EvaluatorFailed {
                name: "anthropic_judge".into(),
                message: "no text in API response".into(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_fails_without_key() {
        // This will fail in CI (no key) -- that's the expected behavior
        if std::env::var("ANTHROPIC_API_KEY").is_err() {
            let result = AnthropicJudgeProvider::from_env();
            assert!(result.is_err());
        }
    }

    #[test]
    fn new_creates_provider() {
        let provider = AnthropicJudgeProvider::new("test-key", "claude-haiku-4-5-20251001");
        assert_eq!(provider.model, "claude-haiku-4-5-20251001");
    }

    #[test]
    #[ignore] // Requires real API key
    fn live_judge_call() {
        let provider = AnthropicJudgeProvider::from_env().unwrap();
        let result = provider.judge(
            "Respond with ONLY a JSON object: {\"score\": 0.8}",
            "Evaluate this: The agent completed the task successfully.",
        );
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(!response.is_empty());
    }
}
