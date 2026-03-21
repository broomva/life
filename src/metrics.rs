//! GenAI and Agent OS metrics instruments.
//!
//! Creates OTel metrics following the GenAI semantic conventions
//! and Life-specific operational metrics.

use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry::{KeyValue, global};
use std::time::Duration;

use crate::semconv;

/// Collection of pre-created GenAI and Agent OS metric instruments.
///
/// Created once via [`GenAiMetrics::new`] and shared across the application.
pub struct GenAiMetrics {
    /// `gen_ai.client.token.usage` — histogram of token counts per request.
    pub token_usage: Histogram<u64>,

    /// `gen_ai.client.operation.duration` — histogram of LLM call duration in seconds.
    pub operation_duration: Histogram<f64>,

    /// `life.tool.executions` — counter of tool executions by name and status.
    pub tool_executions: Counter<u64>,

    /// `life.budget.tokens_remaining` — gauge of remaining token budget.
    pub budget_tokens_remaining: Gauge<u64>,

    /// `life.budget.cost_remaining_usd` — gauge of remaining cost budget.
    pub budget_cost_remaining: Gauge<f64>,

    /// `life.mode.transitions` — counter of operating mode transitions.
    pub mode_transitions: Counter<u64>,

    /// `life.eval.executions` — counter of evaluation executions by evaluator and layer.
    pub eval_executions: Counter<u64>,

    /// `life.eval.score` — histogram of evaluation scores by evaluator.
    pub eval_score: Histogram<f64>,
}

impl GenAiMetrics {
    /// Create all metric instruments from the global meter provider.
    pub fn new(service_name: &'static str) -> Self {
        let meter = global::meter(service_name);
        Self::from_meter(&meter)
    }

    /// Create all metric instruments from a specific meter.
    pub fn from_meter(meter: &Meter) -> Self {
        let token_usage = meter
            .u64_histogram("gen_ai.client.token.usage")
            .with_description("Number of tokens used per GenAI request")
            .with_unit("token")
            .build();

        let operation_duration = meter
            .f64_histogram("gen_ai.client.operation.duration")
            .with_description("Duration of GenAI operations")
            .with_unit("s")
            .with_boundaries(vec![0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0])
            .build();

        let tool_executions = meter
            .u64_counter("life.tool.executions")
            .with_description("Number of tool executions")
            .build();

        let budget_tokens_remaining = meter
            .u64_gauge("life.budget.tokens_remaining")
            .with_description("Remaining token budget")
            .with_unit("token")
            .build();

        let budget_cost_remaining = meter
            .f64_gauge("life.budget.cost_remaining_usd")
            .with_description("Remaining cost budget in USD")
            .with_unit("USD")
            .build();

        let mode_transitions = meter
            .u64_counter("life.mode.transitions")
            .with_description("Number of operating mode transitions")
            .build();

        let eval_executions = meter
            .u64_counter("life.eval.executions")
            .with_description("Number of evaluation executions")
            .build();

        let eval_score = meter
            .f64_histogram("life.eval.score")
            .with_description("Evaluation score distribution")
            .with_boundaries(vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0])
            .build();

        Self {
            token_usage,
            operation_duration,
            tool_executions,
            budget_tokens_remaining,
            budget_cost_remaining,
            mode_transitions,
            eval_executions,
            eval_score,
        }
    }

    /// Record token usage for a GenAI operation.
    pub fn record_token_usage(
        &self,
        model: &str,
        operation: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let base_attrs = [
            KeyValue::new(semconv::GEN_AI_REQUEST_MODEL, model.to_string()),
            KeyValue::new(semconv::GEN_AI_OPERATION_NAME, operation.to_string()),
        ];

        // Record input tokens
        let mut input_attrs = base_attrs.to_vec();
        input_attrs.push(KeyValue::new("gen_ai.token.type", "input"));
        self.token_usage.record(input_tokens, &input_attrs);

        // Record output tokens
        let mut output_attrs = base_attrs.to_vec();
        output_attrs.push(KeyValue::new("gen_ai.token.type", "output"));
        self.token_usage.record(output_tokens, &output_attrs);
    }

    /// Record the duration of a GenAI operation.
    pub fn record_operation_duration(&self, model: &str, operation: &str, duration: Duration) {
        self.operation_duration.record(
            duration.as_secs_f64(),
            &[
                KeyValue::new(semconv::GEN_AI_REQUEST_MODEL, model.to_string()),
                KeyValue::new(semconv::GEN_AI_OPERATION_NAME, operation.to_string()),
            ],
        );
    }

    /// Record a tool execution.
    pub fn record_tool_execution(&self, tool_name: &str, status: &str) {
        self.tool_executions.add(
            1,
            &[
                KeyValue::new(semconv::GEN_AI_TOOL_NAME, tool_name.to_string()),
                KeyValue::new(semconv::LIFE_TOOL_STATUS, status.to_string()),
            ],
        );
    }

    /// Update the remaining budget gauge.
    pub fn record_budget(&self, tokens_remaining: u64, cost_remaining_usd: f64) {
        self.budget_tokens_remaining.record(tokens_remaining, &[]);
        self.budget_cost_remaining.record(cost_remaining_usd, &[]);
    }

    /// Record an evaluation execution.
    pub fn record_eval_execution(&self, evaluator: &str, layer: &str, score: f64) {
        let attrs = [
            KeyValue::new(semconv::LIFE_EVAL_EVALUATOR, evaluator.to_string()),
            KeyValue::new(semconv::LIFE_EVAL_LAYER, layer.to_string()),
        ];
        self.eval_executions.add(1, &attrs);
        self.eval_score.record(score, &attrs);
    }

    /// Record a mode transition.
    pub fn record_mode_transition(&self, from_mode: &str, to_mode: &str) {
        self.mode_transitions.add(
            1,
            &[
                KeyValue::new("life.mode.from", from_mode.to_string()),
                KeyValue::new("life.mode.to", to_mode.to_string()),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_metrics_from_global() {
        // With no meter provider configured, this still works (noop metrics)
        let metrics = GenAiMetrics::new("test-service");

        // All instruments should be usable without panicking
        metrics.record_token_usage("test-model", "chat", 100, 50);
        metrics.record_operation_duration("test-model", "chat", Duration::from_millis(1500));
        metrics.record_tool_execution("read_file", "ok");
        metrics.record_budget(10000, 4.50);
        metrics.record_mode_transition("explore", "execute");
    }

    #[test]
    fn create_metrics_from_meter() {
        let meter = global::meter("test");
        let metrics = GenAiMetrics::from_meter(&meter);
        metrics.record_token_usage("claude", "chat", 200, 100);
    }
}
