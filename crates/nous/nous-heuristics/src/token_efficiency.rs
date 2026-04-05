//! Token efficiency evaluator.
//!
//! Measures the ratio of output tokens to input tokens.
//! A high ratio suggests the model is producing verbose output
//! relative to the prompt size — which may indicate quality issues.

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

/// Evaluates token efficiency: `output_tokens / input_tokens` ratio.
///
/// Score interpretation:
/// - 1.0: ratio <= `ideal_ratio` (compact, efficient)
/// - 0.0: ratio >= `worst_ratio` (extremely verbose)
/// - Linear interpolation between.
pub struct TokenEfficiency {
    /// Ideal output/input ratio (score = 1.0 at or below this).
    ideal_ratio: f64,
    /// Worst acceptable ratio (score = 0.0 at or above this).
    worst_ratio: f64,
}

impl Default for TokenEfficiency {
    fn default() -> Self {
        Self {
            ideal_ratio: 0.5,
            worst_ratio: 3.0,
        }
    }
}

impl NousEvaluator for TokenEfficiency {
    fn name(&self) -> &str {
        "token_efficiency"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Execution
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Inline
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let (Some(input), Some(output)) = (ctx.input_tokens, ctx.output_tokens) else {
            return Ok(vec![]);
        };

        if input == 0 {
            return Ok(vec![]);
        }

        let ratio = output as f64 / input as f64;
        let value = if ratio <= self.ideal_ratio {
            1.0
        } else if ratio >= self.worst_ratio {
            0.0
        } else {
            1.0 - (ratio - self.ideal_ratio) / (self.worst_ratio - self.ideal_ratio)
        };

        let score = EvalScore::new(
            self.name(),
            value,
            self.layer(),
            self.timing(),
            &ctx.session_id,
        )?
        .with_explanation(format!("output/input ratio: {ratio:.2}"));

        Ok(vec![score])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_tokens(input: u64, output: u64) -> EvalContext {
        let mut ctx = EvalContext::new("test");
        ctx.input_tokens = Some(input);
        ctx.output_tokens = Some(output);
        ctx
    }

    #[test]
    fn efficient_output_scores_high() {
        let eval = TokenEfficiency::default();
        let ctx = ctx_with_tokens(1000, 200);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn verbose_output_scores_low() {
        let eval = TokenEfficiency::default();
        let ctx = ctx_with_tokens(100, 400);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!(scores[0].value < 0.5);
    }

    #[test]
    fn extremely_verbose_scores_zero() {
        let eval = TokenEfficiency::default();
        let ctx = ctx_with_tokens(100, 500);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_tokens_returns_empty() {
        let eval = TokenEfficiency::default();
        let ctx = EvalContext::new("test");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn zero_input_returns_empty() {
        let eval = TokenEfficiency::default();
        let ctx = ctx_with_tokens(0, 100);
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }
}
