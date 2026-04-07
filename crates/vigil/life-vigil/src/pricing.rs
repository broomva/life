//! Local pricing model for GenAI cost estimation.
//!
//! Provides a static pricing snapshot for common LLM models, enabling
//! cost estimation without calling provider pricing APIs. Prices are
//! per-million-tokens in USD.
//!
//! # Cost Source
//!
//! All estimates from this module use [`CostSource::EstimatedLocalSnapshot`]
//! provenance — fast but may be stale. Update the snapshot when providers
//! change pricing.

use crate::envelope::{CostSource, LlmResponseEconomics};
use std::time::Duration;

/// Per-model pricing in USD per million tokens.
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    /// Model identifier (matched against `gen_ai.request.model`).
    pub model: &'static str,
    /// Input (prompt) cost per million tokens.
    pub input_per_million: f64,
    /// Output (completion) cost per million tokens.
    pub output_per_million: f64,
}

/// Static pricing snapshot — update when providers change rates.
///
/// Sources: Anthropic pricing page, OpenAI pricing page (as of 2026-04).
/// Prices are in USD per million tokens.
pub const PRICING_SNAPSHOT: &[ModelPricing] = &[
    // ─── Anthropic ─────────────────────────────────────────────────
    ModelPricing {
        model: "claude-opus-4-20250514",
        input_per_million: 15.0,
        output_per_million: 75.0,
    },
    ModelPricing {
        model: "claude-sonnet-4-20250514",
        input_per_million: 3.0,
        output_per_million: 15.0,
    },
    ModelPricing {
        model: "claude-sonnet-4-5-20250929",
        input_per_million: 3.0,
        output_per_million: 15.0,
    },
    ModelPricing {
        model: "claude-haiku-4-5-20251001",
        input_per_million: 0.80,
        output_per_million: 4.0,
    },
    // ─── OpenAI ────────────────────────────────────────────────────
    ModelPricing {
        model: "gpt-4o",
        input_per_million: 2.50,
        output_per_million: 10.0,
    },
    ModelPricing {
        model: "gpt-4o-mini",
        input_per_million: 0.15,
        output_per_million: 0.60,
    },
    ModelPricing {
        model: "o3",
        input_per_million: 10.0,
        output_per_million: 40.0,
    },
    ModelPricing {
        model: "o3-mini",
        input_per_million: 1.10,
        output_per_million: 4.40,
    },
    ModelPricing {
        model: "o4-mini",
        input_per_million: 1.10,
        output_per_million: 4.40,
    },
    // ─── OpenRouter / proxy models (common patterns) ───────────────
    ModelPricing {
        model: "anthropic/claude-haiku-4.5",
        input_per_million: 0.80,
        output_per_million: 4.0,
    },
    ModelPricing {
        model: "anthropic/claude-sonnet-4",
        input_per_million: 3.0,
        output_per_million: 15.0,
    },
    ModelPricing {
        model: "anthropic/claude-opus-4",
        input_per_million: 15.0,
        output_per_million: 75.0,
    },
];

/// Look up pricing for a model by name.
///
/// Tries exact match first, then substring match (e.g. "claude-sonnet-4"
/// matches "anthropic/claude-sonnet-4" or a dated variant).
pub fn lookup_pricing(model: &str) -> Option<&'static ModelPricing> {
    // Exact match first.
    if let Some(p) = PRICING_SNAPSHOT.iter().find(|p| p.model == model) {
        return Some(p);
    }
    // Substring match: model name contains a known pricing key.
    PRICING_SNAPSHOT
        .iter()
        .find(|p| model.contains(p.model) || p.model.contains(model))
}

/// Estimate USD cost for a given token count.
///
/// Returns `(input_cost, output_cost, total_cost)` or `None` if the
/// model is not in the pricing snapshot.
pub fn estimate_cost(
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> Option<(f64, f64, f64)> {
    let pricing = lookup_pricing(model)?;
    let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_per_million;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_per_million;
    Some((input_cost, output_cost, input_cost + output_cost))
}

/// Build an `LlmResponseEconomics` from token counts and a model name.
///
/// Uses the local pricing snapshot for cost estimation. If the model is not
/// found, costs are set to `None` but token counts are still recorded.
pub fn build_response_economics(
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    duration: Duration,
) -> LlmResponseEconomics {
    let costs = estimate_cost(model, input_tokens, output_tokens);

    LlmResponseEconomics {
        cost_source: CostSource::EstimatedLocalSnapshot,
        input_tokens,
        output_tokens,
        total_tokens: input_tokens + output_tokens,
        input_cost_usd: costs.map(|(i, _, _)| i),
        output_cost_usd: costs.map(|(_, o, _)| o),
        total_cost_usd: costs.map(|(_, _, t)| t),
        cache_read_tokens,
        cache_creation_tokens,
        duration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_exact_match() {
        let p = lookup_pricing("claude-sonnet-4-20250514").unwrap();
        assert!((p.input_per_million - 3.0).abs() < 0.001);
        assert!((p.output_per_million - 15.0).abs() < 0.001);
    }

    #[test]
    fn lookup_substring_match() {
        // Proxy-style model names should match
        let p = lookup_pricing("anthropic/claude-haiku-4.5").unwrap();
        assert!((p.input_per_million - 0.80).abs() < 0.001);
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup_pricing("unknown-model-xyz").is_none());
    }

    #[test]
    fn estimate_cost_claude_sonnet() {
        let (input, output, total) = estimate_cost("claude-sonnet-4-20250514", 1000, 500).unwrap();
        // 1000 input tokens at $3/M = $0.003
        assert!((input - 0.003).abs() < 0.0001);
        // 500 output tokens at $15/M = $0.0075
        assert!((output - 0.0075).abs() < 0.0001);
        assert!((total - 0.0105).abs() < 0.0001);
    }

    #[test]
    fn estimate_cost_gpt4o() {
        let (input, output, total) = estimate_cost("gpt-4o", 10_000, 2_000).unwrap();
        // 10k input at $2.50/M = $0.025
        assert!((input - 0.025).abs() < 0.001);
        // 2k output at $10/M = $0.02
        assert!((output - 0.02).abs() < 0.001);
        assert!((total - 0.045).abs() < 0.001);
    }

    #[test]
    fn estimate_cost_unknown_model() {
        assert!(estimate_cost("llama-local", 100, 50).is_none());
    }

    #[test]
    fn build_response_economics_known_model() {
        let econ = build_response_economics(
            "claude-sonnet-4-20250514",
            1000,
            500,
            0,
            0,
            Duration::from_millis(800),
        );
        assert_eq!(econ.cost_source, CostSource::EstimatedLocalSnapshot);
        assert_eq!(econ.input_tokens, 1000);
        assert_eq!(econ.output_tokens, 500);
        assert_eq!(econ.total_tokens, 1500);
        assert!(econ.total_cost_usd.is_some());
        assert!((econ.total_cost_usd.unwrap() - 0.0105).abs() < 0.0001);
    }

    #[test]
    fn build_response_economics_unknown_model() {
        let econ =
            build_response_economics("local-llama", 100, 50, 0, 0, Duration::from_millis(200));
        assert_eq!(econ.cost_source, CostSource::EstimatedLocalSnapshot);
        assert_eq!(econ.input_tokens, 100);
        assert!(econ.total_cost_usd.is_none());
    }

    #[test]
    fn zero_tokens_zero_cost() {
        let (input, output, total) = estimate_cost("gpt-4o", 0, 0).unwrap();
        assert!(input.abs() < f64::EPSILON);
        assert!(output.abs() < f64::EPSILON);
        assert!(total.abs() < f64::EPSILON);
    }
}
