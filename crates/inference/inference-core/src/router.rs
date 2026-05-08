//! Router that selects an [`crate::InferenceBackend`] per call.
//!
//! Per L5-D7 routing is dynamic: a single agent loop may visit
//! multiple backends (small drafter for routing, large model for
//! synthesis). Static defaults live in policy; Autonomic can override
//! at runtime via [`InferenceRouter::set_policy`].

use std::sync::Arc;
use std::time::Instant;

use crate::backend::InferenceBackend;
use crate::ids::ModelId;

/// Workload classification fed to the router. Maps loosely to phases
/// of the agent loop the reel describes (memory-bound model calls,
/// I/O-bound tool use, CPU-bound orchestration). Backends self-describe
/// where they're best.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkloadClass {
    /// Small / fast model picking the next action.
    Routing,
    /// Large model producing user-facing output.
    Synthesis,
    /// Step expected to emit a tool call. Wants low TTFT.
    ToolEmit,
    /// Embedding generation (vector output, no token stream).
    Embed,
}

/// Routing inputs. Cheap to construct per-call.
pub struct RoutingHint {
    /// Requested model.
    pub model: ModelId,
    /// Workload class (drives latency/throughput/cost trade-offs).
    pub workload: WorkloadClass,
    /// Optional wall-clock cutoff.
    pub deadline: Option<Instant>,
}

/// Routing strategy. E-Sub-A ships two: `single` (always pick the
/// only backend), and `strict_model_match` (pick the first backend
/// whose `capabilities().supported_models` contains the requested
/// model). Production policies (cost-aware, latency-aware,
/// Autonomic-driven) are E-Sub-E.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferencePolicy {
    /// Always return the first backend; ignore hint contents.
    Single,
    /// Pick the first backend whose capabilities advertise the model.
    StrictModelMatch,
}

impl InferencePolicy {
    /// Construct the [`InferencePolicy::Single`] policy.
    #[must_use]
    pub fn single() -> Self {
        Self::Single
    }
    /// Construct the [`InferencePolicy::StrictModelMatch`] policy.
    #[must_use]
    pub fn strict_model_match() -> Self {
        Self::StrictModelMatch
    }
}

/// Routing error.
#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    /// No backend in the router advertises the requested model.
    #[error("no backend supports model {0}")]
    NoBackendForModel(ModelId),
    /// The router was constructed with an empty backend list.
    #[error("router has no backends")]
    NoBackends,
}

/// Routes [`RoutingHint`]s to one of the configured backends.
pub struct InferenceRouter {
    backends: Vec<Arc<dyn InferenceBackend>>,
    policy: InferencePolicy,
}

impl InferenceRouter {
    /// Construct a router with the given backends and policy.
    #[must_use]
    pub fn new(backends: Vec<Arc<dyn InferenceBackend>>, policy: InferencePolicy) -> Self {
        Self { backends, policy }
    }

    /// Pick a backend for `hint`. Returns [`RouteError`] if no backend
    /// applies.
    ///
    /// # Errors
    /// Returns [`RouteError::NoBackends`] if the router was constructed
    /// with an empty backend list, or [`RouteError::NoBackendForModel`]
    /// when no backend advertises the requested model under the
    /// `StrictModelMatch` policy.
    pub fn route(&self, hint: &RoutingHint) -> Result<&Arc<dyn InferenceBackend>, RouteError> {
        if self.backends.is_empty() {
            return Err(RouteError::NoBackends);
        }
        match self.policy {
            InferencePolicy::Single => Ok(&self.backends[0]),
            InferencePolicy::StrictModelMatch => self
                .backends
                .iter()
                .find(|b| b.capabilities().supported_models.contains(&hint.model))
                .ok_or_else(|| RouteError::NoBackendForModel(hint.model.clone())),
        }
    }

    /// Replace the routing policy. Autonomic uses this to retune.
    pub fn set_policy(&mut self, policy: InferencePolicy) {
        self.policy = policy;
    }
}
