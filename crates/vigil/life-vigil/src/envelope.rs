//! Typed LLM request envelope for structured cost attribution.
//!
//! Inspired by Turnpike's `LLMRequestEnvelope` — a frozen dataclass with
//! 6 semantic blocks that travels with every provider call. Provides:
//!
//! - **CostSource provenance**: whether a cost is estimated, API-derived, or actual
//! - **Dual-write ready**: OTel span attributes + JSONL local artifacts
//! - **Reliability tracking**: retry count, fallback state, circuit breaker
//!
//! # Semantic Blocks
//!
//! 1. **Identity** — session, run, agent, step
//! 2. **Model Selection** — provider, model, sampling parameters
//! 3. **Economics** — cost estimates, budget remaining
//! 4. **Reliability** — retries, fallback, circuit state
//! 5. **Governance** — tool whitelist, approval gates
//! 6. **Cache / Eval** — cache key, eval run metadata

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::semconv;

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

// ─── CostSource Provenance ─────────────────────────────────────────────────

/// Provenance of a cost figure — tracks WHERE the number came from.
///
/// Downstream consumers use this to decide confidence level:
/// - `Actual` can be billed directly.
/// - `EstimatedProviderApi` is reliable but may drift.
/// - `EstimatedLocalSnapshot` is fast but may be stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    /// Estimated from a local pricing snapshot (may be stale).
    EstimatedLocalSnapshot,
    /// Estimated using the provider's pricing API.
    EstimatedProviderApi,
    /// Actual cost reported by the provider in the response.
    Actual,
}

// ─── Circuit Breaker State ─────────────────────────────────────────────────

/// Circuit breaker state for reliability tracking (BRO-519).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    #[default]
    Closed,
    /// Circuit tripped — requests are blocked.
    Open,
    /// Probing — allowing a single request to test recovery.
    HalfOpen,
}

// ─── LLM Request Envelope ──────────────────────────────────────────────────

/// Frozen envelope capturing all context for an LLM provider call.
///
/// Created before each provider call and enriched after the response.
/// Carries the 6 semantic blocks that enable structured cost attribution,
/// reliability tracking, and governance auditing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequestEnvelope {
    // ── Block 1: Identity ──────────────────────────────────────────
    /// Stable request ID for this provider call.
    #[serde(default)]
    pub request_id: String,
    /// Agent OS session ID.
    pub session_id: String,
    /// Agent OS branch ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
    /// Run ID within the session.
    pub run_id: String,
    /// Agent name (e.g. "arcan").
    pub agent_name: String,
    /// Step index within the current run (0-based).
    pub step_index: u32,
    /// Tenant identifier for multi-tenant attribution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    /// Caller identifier for actor-level attribution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<String>,
    /// Task identifier for task-level attribution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,

    // ── Block 2: Model Selection ───────────────────────────────────
    /// Provider name (e.g. "anthropic", "openai").
    pub provider: String,
    /// Provider requested before routing.
    #[serde(default)]
    pub provider_requested: String,
    /// Provider selected after routing.
    #[serde(default)]
    pub provider_selected: String,
    /// Model identifier (e.g. "claude-sonnet-4-20250514").
    pub model: String,
    /// Model tier used by routing and policy decisions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_tier: Option<String>,
    /// Routing decision that selected the provider/model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_decision: Option<String>,
    /// Maximum tokens for model response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Top-p (nucleus) sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    // ── Block 3: Economics ─────────────────────────────────────────
    /// How the cost estimate was derived (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_source: Option<CostSource>,
    /// Estimated input cost in USD (pre-call).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_input_cost_usd: Option<f64>,
    /// Estimated output cost in USD (pre-call).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_output_cost_usd: Option<f64>,
    /// Estimated total cost in USD (pre-call, input + output).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_total_cost_usd: Option<f64>,
    /// Estimated pre-call input tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u32>,
    /// Estimated pre-call output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u32>,
    /// Alias for estimated total cost used by downstream dashboards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
    /// Remaining token budget before this call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens_remaining: Option<u64>,
    /// Remaining USD budget before this call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_cost_remaining_usd: Option<f64>,

    // ── Block 4: Reliability ───────────────────────────────────────
    /// Number of retries attempted before this call succeeded.
    #[serde(default)]
    pub retry_count: u32,
    /// Whether a fallback provider was used.
    #[serde(default)]
    pub fallback_triggered: bool,
    /// Why fallback occurred, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    /// Circuit breaker state at time of call.
    #[serde(default)]
    pub circuit_state: CircuitState,
    /// Provider call latency in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Time to first streamed token in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<u64>,
    /// Provider-native finish reason, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Request timeout (if set).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "option_duration_secs"
    )]
    pub timeout: Option<Duration>,

    // ── Block 5: Governance ────────────────────────────────────────
    /// Tool whitelist from active skill (None = all tools allowed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Whether human approval is required for this call.
    #[serde(default)]
    pub approval_required: bool,
    /// Provider policy decision applied at the request boundary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_decision: Option<String>,
    /// Policy mode active for the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_mode: Option<String>,
    /// Whether PII was detected before sending content to the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pii_detected: Option<bool>,
    /// Whether provider-bound content was redacted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction_applied: Option<bool>,

    // ── Block 6: Cache / Eval ──────────────────────────────────────
    /// Cache key for prompt deduplication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    /// Eval run ID (when this call is part of an evaluation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_run_id: Option<String>,
}

impl LlmRequestEnvelope {
    /// Create a minimal envelope with identity + model selection.
    ///
    /// Economics, reliability, governance, and cache fields default to None/zero.
    pub fn new(
        session_id: impl Into<String>,
        run_id: impl Into<String>,
        agent_name: impl Into<String>,
        step_index: u32,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let session_id = session_id.into();
        let run_id = run_id.into();
        let agent_name = agent_name.into();
        let provider = provider.into();
        let model = model.into();
        let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let request_id =
            format!("{session_id}:{run_id}:{step_index}:{provider}:{model}:{sequence}");

        Self {
            request_id,
            session_id,
            branch_id: None,
            run_id,
            agent_name,
            step_index,
            tenant_id: None,
            caller_id: None,
            task_id: None,
            provider: provider.clone(),
            provider_requested: provider.clone(),
            provider_selected: provider,
            model,
            model_tier: None,
            routing_decision: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            cost_source: None,
            estimated_input_cost_usd: None,
            estimated_output_cost_usd: None,
            estimated_total_cost_usd: None,
            tokens_in: None,
            tokens_out: None,
            estimated_cost_usd: None,
            budget_tokens_remaining: None,
            budget_cost_remaining_usd: None,
            retry_count: 0,
            fallback_triggered: false,
            fallback_reason: None,
            circuit_state: CircuitState::default(),
            latency_ms: None,
            time_to_first_token_ms: None,
            finish_reason: None,
            timeout: None,
            allowed_tools: None,
            approval_required: false,
            policy_decision: None,
            policy_mode: None,
            pii_detected: None,
            redaction_applied: None,
            cache_key: None,
            eval_run_id: None,
        }
    }

    /// Emit the envelope's key fields as OTel span attributes on the given span.
    ///
    /// Records identity, model selection, and economics attributes.
    /// Reliability and governance are emitted only when non-default.
    pub fn record_on_span(&self, span: &tracing::Span) {
        if !self.request_id.is_empty() {
            span.record(semconv::VIGIL_LLM_REQUEST_ID, self.request_id.as_str());
        }
        span.record(semconv::LIFE_SESSION_ID, self.session_id.as_str());
        if let Some(ref branch_id) = self.branch_id {
            span.record(semconv::LIFE_BRANCH_ID, branch_id.as_str());
        }
        span.record(semconv::LIFE_RUN_ID, self.run_id.as_str());
        span.record(semconv::GEN_AI_AGENT_NAME, self.agent_name.as_str());
        span.record(semconv::GEN_AI_SYSTEM, self.provider.as_str());
        if let Some(ref tenant_id) = self.tenant_id {
            span.record(semconv::VIGIL_LLM_TENANT_ID, tenant_id.as_str());
        }
        if let Some(ref caller_id) = self.caller_id {
            span.record(semconv::VIGIL_LLM_CALLER_ID, caller_id.as_str());
        }
        if let Some(ref task_id) = self.task_id {
            span.record(semconv::VIGIL_LLM_TASK_ID, task_id.as_str());
        }
        if !self.provider_requested.is_empty() {
            span.record(
                semconv::VIGIL_LLM_PROVIDER_REQUESTED,
                self.provider_requested.as_str(),
            );
        }
        if !self.provider_selected.is_empty() {
            span.record(
                semconv::VIGIL_LLM_PROVIDER_SELECTED,
                self.provider_selected.as_str(),
            );
        }
        span.record(semconv::GEN_AI_REQUEST_MODEL, self.model.as_str());
        if let Some(ref model_tier) = self.model_tier {
            span.record(semconv::VIGIL_LLM_MODEL_TIER, model_tier.as_str());
        }
        if let Some(ref routing_decision) = self.routing_decision {
            span.record(
                semconv::VIGIL_LLM_ROUTING_DECISION,
                routing_decision.as_str(),
            );
        }
        if let Some(max_tokens) = self.max_tokens {
            span.record(semconv::GEN_AI_REQUEST_MAX_TOKENS, max_tokens);
        }
        if let Some(temperature) = self.temperature {
            span.record(semconv::GEN_AI_REQUEST_TEMPERATURE, temperature);
        }
        if let Some(top_p) = self.top_p {
            span.record(semconv::GEN_AI_REQUEST_TOP_P, top_p);
        }
        if let Some(tokens_in) = self.tokens_in {
            span.record(semconv::VIGIL_LLM_TOKENS_IN, tokens_in);
        }
        if let Some(tokens_out) = self.tokens_out {
            span.record(semconv::VIGIL_LLM_TOKENS_OUT, tokens_out);
        }
        if let Some(cost_source) = self.cost_source {
            span.record(
                semconv::VIGIL_LLM_COST_SOURCE,
                serde_cost_source(cost_source),
            );
        }
        if let Some(estimated_cost) = self.estimated_cost_usd.or(self.estimated_total_cost_usd) {
            span.record(semconv::VIGIL_LLM_ESTIMATED_COST_USD, estimated_cost);
        }

        if let Some(budget_tokens) = self.budget_tokens_remaining {
            span.record(semconv::LIFE_BUDGET_TOKENS, budget_tokens);
        }
        if let Some(budget_cost) = self.budget_cost_remaining_usd {
            span.record(semconv::LIFE_BUDGET_COST, budget_cost);
        }
        span.record(semconv::LIFE_RETRY_COUNT, self.retry_count);
        span.record(semconv::LIFE_FALLBACK_TRIGGERED, self.fallback_triggered);
        span.record(
            semconv::LIFE_CIRCUIT_STATE,
            serde_circuit_state(self.circuit_state),
        );
        span.record(semconv::VIGIL_LLM_RETRY_COUNT, self.retry_count);
        span.record(
            semconv::VIGIL_LLM_FALLBACK_TRIGGERED,
            self.fallback_triggered,
        );
        if let Some(ref fallback_reason) = self.fallback_reason {
            span.record(semconv::VIGIL_LLM_FALLBACK_REASON, fallback_reason.as_str());
        }
        span.record(
            semconv::VIGIL_LLM_CIRCUIT_STATE,
            serde_circuit_state(self.circuit_state),
        );
        if let Some(latency_ms) = self.latency_ms {
            span.record(semconv::VIGIL_LLM_LATENCY_MS, latency_ms);
        }
        if let Some(ttft_ms) = self.time_to_first_token_ms {
            span.record(semconv::VIGIL_LLM_TTFT_MS, ttft_ms);
        }
        if let Some(ref finish_reason) = self.finish_reason {
            span.record(semconv::VIGIL_LLM_FINISH_REASON, finish_reason.as_str());
        }
        if let Some(ref policy_decision) = self.policy_decision {
            span.record(semconv::VIGIL_LLM_POLICY_DECISION, policy_decision.as_str());
        }
        if let Some(ref policy_mode) = self.policy_mode {
            span.record(semconv::VIGIL_LLM_POLICY_MODE, policy_mode.as_str());
        }
        if let Some(pii_detected) = self.pii_detected {
            span.record(semconv::VIGIL_LLM_PII_DETECTED, pii_detected);
        }
        if let Some(redaction_applied) = self.redaction_applied {
            span.record(semconv::VIGIL_LLM_REDACTION_APPLIED, redaction_applied);
        }
    }
}

// ─── Response Economics ────────────────────────────────────────────────────

/// Response-side economics captured after a provider call completes.
///
/// Enriches the envelope with actual token counts, costs, and timing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponseEconomics {
    /// How the cost was determined.
    pub cost_source: CostSource,
    /// Input (prompt) tokens consumed.
    pub input_tokens: u32,
    /// Output (completion) tokens generated.
    pub output_tokens: u32,
    /// Total tokens (input + output).
    pub total_tokens: u32,
    /// Input cost in USD (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cost_usd: Option<f64>,
    /// Output cost in USD (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_cost_usd: Option<f64>,
    /// Total cost in USD (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    /// Cache read tokens (Anthropic prompt caching).
    #[serde(default)]
    pub cache_read_tokens: u32,
    /// Cache creation tokens (Anthropic prompt caching).
    #[serde(default)]
    pub cache_creation_tokens: u32,
    /// Wall-clock duration of the provider call.
    #[serde(with = "duration_secs")]
    pub duration: Duration,
}

impl LlmResponseEconomics {
    /// Emit response-side economics on the given span.
    pub fn record_on_span(&self, span: &tracing::Span) {
        span.record(semconv::GEN_AI_USAGE_INPUT_TOKENS, self.input_tokens);
        span.record(semconv::GEN_AI_USAGE_OUTPUT_TOKENS, self.output_tokens);
        span.record(
            semconv::VIGIL_LLM_COST_SOURCE,
            serde_cost_source(self.cost_source),
        );
        if let Some(total_cost) = self.total_cost_usd {
            span.record(semconv::VIGIL_LLM_ESTIMATED_COST_USD, total_cost);
        }
        let latency_ms = self.duration.as_millis().min(u128::from(u64::MAX)) as u64;
        span.record(semconv::VIGIL_LLM_LATENCY_MS, latency_ms);
    }
}

fn serde_cost_source(source: CostSource) -> &'static str {
    match source {
        CostSource::EstimatedLocalSnapshot => "estimated_local_snapshot",
        CostSource::EstimatedProviderApi => "estimated_provider_api",
        CostSource::Actual => "actual",
    }
}

fn serde_circuit_state(state: CircuitState) -> &'static str {
    match state {
        CircuitState::Closed => "closed",
        CircuitState::Open => "open",
        CircuitState::HalfOpen => "half_open",
    }
}

// ─── Duration serde helpers ────────────────────────────────────────────────

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(d.as_secs_f64())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = f64::deserialize(d)?;
        Ok(Duration::from_secs_f64(secs))
    }
}

mod option_duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match d {
            Some(d) => s.serialize_f64(d.as_secs_f64()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        let opt = Option::<f64>::deserialize(d)?;
        Ok(opt.map(Duration::from_secs_f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_new_has_sensible_defaults() {
        let env = LlmRequestEnvelope::new("sess-1", "run-1", "arcan", 0, "anthropic", "claude");
        assert_eq!(env.session_id, "sess-1");
        assert!(
            env.request_id
                .starts_with("sess-1:run-1:0:anthropic:claude:")
        );
        assert_eq!(env.provider, "anthropic");
        assert_eq!(env.retry_count, 0);
        assert!(!env.fallback_triggered);
        assert_eq!(env.circuit_state, CircuitState::Closed);
        assert!(env.cost_source.is_none());
        assert!(env.allowed_tools.is_none());
    }

    #[test]
    fn envelope_serialization_round_trip() {
        let env = LlmRequestEnvelope::new("s1", "r1", "arcan", 3, "openai", "gpt-4o");
        let json = serde_json::to_string(&env).unwrap();
        let deserialized: LlmRequestEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.session_id, "s1");
        assert_eq!(deserialized.step_index, 3);
        assert_eq!(deserialized.model, "gpt-4o");
    }

    #[test]
    fn envelope_request_id_is_unique_per_call() {
        let first = LlmRequestEnvelope::new("s1", "r1", "arcan", 3, "openai", "gpt-4o");
        let second = LlmRequestEnvelope::new("s1", "r1", "arcan", 3, "openai", "gpt-4o");

        assert_ne!(first.request_id, second.request_id);
        assert!(first.request_id.starts_with("s1:r1:3:openai:gpt-4o:"));
        assert!(second.request_id.starts_with("s1:r1:3:openai:gpt-4o:"));
    }

    #[test]
    fn envelope_skip_serializing_none_fields() {
        let env = LlmRequestEnvelope::new("s1", "r1", "arcan", 0, "anthropic", "claude");
        let json = serde_json::to_string(&env).unwrap();
        // None fields should be omitted
        assert!(!json.contains("max_tokens"));
        assert!(!json.contains("temperature"));
        assert!(!json.contains("cost_source"));
        assert!(!json.contains("cache_key"));
    }

    #[test]
    fn envelope_with_economics() {
        let mut env = LlmRequestEnvelope::new("s1", "r1", "arcan", 0, "anthropic", "claude");
        env.cost_source = Some(CostSource::EstimatedLocalSnapshot);
        env.estimated_total_cost_usd = Some(0.003);
        env.budget_tokens_remaining = Some(50_000);
        env.budget_cost_remaining_usd = Some(1.50);

        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("estimated_local_snapshot"));
        assert!(json.contains("0.003"));
    }

    #[test]
    fn response_economics_serialization() {
        let econ = LlmResponseEconomics {
            cost_source: CostSource::Actual,
            input_tokens: 150,
            output_tokens: 50,
            total_tokens: 200,
            input_cost_usd: Some(0.00015),
            output_cost_usd: Some(0.00075),
            total_cost_usd: Some(0.0009),
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            duration: Duration::from_millis(1250),
        };

        let json = serde_json::to_string(&econ).unwrap();
        let deserialized: LlmResponseEconomics = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.input_tokens, 150);
        assert_eq!(deserialized.cost_source, CostSource::Actual);
        assert!((deserialized.duration.as_secs_f64() - 1.25).abs() < 0.001);
    }

    #[test]
    fn cost_source_variants() {
        let local: CostSource = serde_json::from_str("\"estimated_local_snapshot\"").unwrap();
        assert_eq!(local, CostSource::EstimatedLocalSnapshot);

        let api: CostSource = serde_json::from_str("\"estimated_provider_api\"").unwrap();
        assert_eq!(api, CostSource::EstimatedProviderApi);

        let actual: CostSource = serde_json::from_str("\"actual\"").unwrap();
        assert_eq!(actual, CostSource::Actual);
    }

    #[test]
    fn circuit_state_default_is_closed() {
        assert_eq!(CircuitState::default(), CircuitState::Closed);
    }

    #[test]
    fn circuit_state_serialization() {
        let open: CircuitState = serde_json::from_str("\"open\"").unwrap();
        assert_eq!(open, CircuitState::Open);
        let half: CircuitState = serde_json::from_str("\"half_open\"").unwrap();
        assert_eq!(half, CircuitState::HalfOpen);
    }

    #[test]
    fn envelope_with_reliability() {
        let mut env = LlmRequestEnvelope::new("s1", "r1", "arcan", 0, "openai", "gpt-4o");
        env.retry_count = 2;
        env.fallback_triggered = true;
        env.fallback_reason = Some("primary_timeout".to_owned());
        env.circuit_state = CircuitState::HalfOpen;
        env.time_to_first_token_ms = Some(123);
        env.finish_reason = Some("stop".to_owned());
        env.timeout = Some(Duration::from_secs(30));

        let json = serde_json::to_string(&env).unwrap();
        let deserialized: LlmRequestEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.retry_count, 2);
        assert!(deserialized.fallback_triggered);
        assert_eq!(
            deserialized.fallback_reason.as_deref(),
            Some("primary_timeout")
        );
        assert_eq!(deserialized.circuit_state, CircuitState::HalfOpen);
        assert_eq!(deserialized.time_to_first_token_ms, Some(123));
        assert_eq!(deserialized.finish_reason.as_deref(), Some("stop"));
        assert_eq!(deserialized.timeout, Some(Duration::from_secs(30)));
    }
}
