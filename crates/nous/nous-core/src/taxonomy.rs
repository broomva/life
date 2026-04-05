//! Evaluation layer taxonomy.
//!
//! Categorizes evaluators into distinct layers of agent behavior.
//! Each layer measures a different aspect of quality.

use serde::{Deserialize, Serialize};

/// The layer of agent behavior being evaluated.
///
/// Each evaluator belongs to exactly one layer. Layers enable
/// aggregation and filtering of scores by concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalLayer {
    /// Reasoning quality — coherence, completeness, logical soundness.
    Reasoning,
    /// Action quality — tool usage correctness, argument validity.
    Action,
    /// Execution quality — efficiency, iteration count, token usage.
    Execution,
    /// Safety — policy compliance, blocklist checks, capability enforcement.
    Safety,
    /// Cost — budget adherence, spend velocity, resource efficiency.
    Cost,
}

impl EvalLayer {
    /// Human-readable label for the layer.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::Action => "action",
            Self::Execution => "execution",
            Self::Safety => "safety",
            Self::Cost => "cost",
        }
    }
}

impl std::fmt::Display for EvalLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// When in the agent lifecycle an evaluator runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalTiming {
    /// Runs inline in the middleware hook (< 2ms budget).
    Inline,
    /// Runs asynchronously after the hook returns.
    Async,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_layer_display() {
        assert_eq!(EvalLayer::Reasoning.to_string(), "reasoning");
        assert_eq!(EvalLayer::Action.to_string(), "action");
        assert_eq!(EvalLayer::Execution.to_string(), "execution");
        assert_eq!(EvalLayer::Safety.to_string(), "safety");
        assert_eq!(EvalLayer::Cost.to_string(), "cost");
    }

    #[test]
    fn eval_layer_serde_roundtrip() {
        let layer = EvalLayer::Reasoning;
        let json = serde_json::to_string(&layer).unwrap();
        assert_eq!(json, "\"reasoning\"");
        let back: EvalLayer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layer);
    }

    #[test]
    fn eval_timing_serde_roundtrip() {
        let timing = EvalTiming::Inline;
        let json = serde_json::to_string(&timing).unwrap();
        let back: EvalTiming = serde_json::from_str(&json).unwrap();
        assert_eq!(back, timing);
    }

    #[test]
    fn all_layers_have_labels() {
        let layers = [
            EvalLayer::Reasoning,
            EvalLayer::Action,
            EvalLayer::Execution,
            EvalLayer::Safety,
            EvalLayer::Cost,
        ];
        for layer in layers {
            assert!(!layer.label().is_empty());
        }
    }
}
