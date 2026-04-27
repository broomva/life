//! Saga driver per Spec C₂ §4.1.
//!
//! Runs `Vec<Box<dyn SagaStep>>` left-to-right; on first step error,
//! invokes `compensate` on each previously-completed step in reverse order.
//! Compensation failures are logged but not retried (per spec, the saga
//! must not loop on compensation pathologies — operators force-recover via
//! the admin plane in sub-phase C).
//!
//! Sub-phase C wires:
//! - In-memory `SagaRegistry` so admin-plane `Saga.ListInflight` and
//!   `Saga.Show` have a reader.
//! - Best-effort lago persistence to `system/lifed/saga/<saga_id>` via a
//!   `SagaJournal` trait so historical sagas survive lifed restarts. The
//!   in-memory implementation drops events on the floor; the lago-backed
//!   implementation appends to lago's `idem_persist` (sub-phase C MVS;
//!   sub-phase D2 swaps to a typed `lago.append_event` once that RPC
//!   ships in lago-proxy).

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use aios_proto::aios::v1 as aios_v1;

use crate::auth::capability::CapabilityClaims;
use crate::saga::registry::{SagaRegistry, SagaStatus};

#[derive(Debug, Error)]
pub enum SagaError {
    #[error("step '{name}' forward: {source}")]
    Forward {
        name: &'static str,
        source: tonic::Status,
    },
    #[error("step '{name}' compensate: {source}")]
    Compensate {
        name: &'static str,
        source: tonic::Status,
    },
    #[error("deadline exceeded for saga {kind}")]
    Deadline { kind: &'static str },
}

impl SagaError {
    pub fn into_status(self) -> tonic::Status {
        match self {
            SagaError::Forward { source, .. } => source,
            SagaError::Compensate { source, .. } => source,
            SagaError::Deadline { kind } => {
                tonic::Status::deadline_exceeded(format!("saga {kind} timed out"))
            }
        }
    }
}

pub struct SagaCtx {
    pub saga_id: String,
    pub user_id: String,
    pub project_id: String,
    pub sid: aios_v1::SessionId,
    pub idempotency_key: Option<String>,
    pub deadline: Instant,
    pub trace: tracing::Span,
    pub claims: CapabilityClaims,
}

#[async_trait]
pub trait SagaStep: Send + Sync {
    async fn forward(&self, ctx: &SagaCtx) -> Result<(), SagaError>;
    async fn compensate(&self, ctx: &SagaCtx) -> Result<(), SagaError>;
    fn name(&self) -> &'static str;
}

/// Trait for persisting saga lifecycle events to lago. Spec C₂ §4.1
/// requires every saga state transition lands in
/// `system/lifed/saga/<saga_id>`. We expose a small trait so the saga
/// driver doesn't take a hard dependency on `LagoCall`.
#[async_trait]
pub trait SagaJournal: Send + Sync {
    /// Append one saga lifecycle event. Errors are logged by the caller
    /// (saga driver) but never propagate — persistence is best-effort
    /// per Spec C₂ §4.1 (the in-memory `SagaRegistry` is the source of
    /// truth for live introspection).
    async fn append_event(&self, saga_id: &str, event: SagaEvent) -> Result<(), tonic::Status>;
}

/// One persisted saga lifecycle event. The shape is stable across
/// in-memory + lago backends so backfill is straightforward.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SagaEvent {
    pub event_type: SagaEventType,
    pub saga_kind: String,
    pub sid: String,
    pub step: Option<String>,
    pub seq: u32,
    pub timestamp_ms: i64,
}

/// The five saga lifecycle event types per Spec C₂ §4.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SagaEventType {
    Started,
    StepForward,
    StepCompensated,
    Completed,
    Failed,
}

impl SagaEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SagaEventType::Started => "saga.started",
            SagaEventType::StepForward => "saga.step_forward",
            SagaEventType::StepCompensated => "saga.step_compensated",
            SagaEventType::Completed => "saga.completed",
            SagaEventType::Failed => "saga.failed",
        }
    }
}

/// In-memory journal — drops every event. Used by tests + dev daemon
/// where lago isn't in the loop.
#[derive(Default, Clone)]
pub struct InMemorySagaJournal;

#[async_trait]
impl SagaJournal for InMemorySagaJournal {
    async fn append_event(&self, _saga_id: &str, _event: SagaEvent) -> Result<(), tonic::Status> {
        Ok(())
    }
}

/// Lago-backed journal — appends every event to
/// `system/lifed/saga/<saga_id>` per Spec C₂ §4.1. The payload is
/// JSON-encoded `SagaEvent`. Sub-phase C uses lago-proxy's best-effort
/// `append_event` shim; sub-phase D2 wires the typed `lago.Append` RPC.
pub struct LagoSagaJournal {
    pub lago: Arc<dyn lago_proxy::LagoCall>,
}

impl LagoSagaJournal {
    pub fn new(lago: Arc<dyn lago_proxy::LagoCall>) -> Self {
        Self { lago }
    }
}

#[async_trait]
impl SagaJournal for LagoSagaJournal {
    async fn append_event(&self, saga_id: &str, event: SagaEvent) -> Result<(), tonic::Status> {
        let namespace = format!("system/lifed/saga/{saga_id}");
        let payload = serde_json::to_vec(&event)
            .map_err(|e| tonic::Status::internal(format!("saga event encode: {e}")))?;
        self.lago
            .append_event(&namespace, event.event_type.as_str(), payload)
            .await
            .map_err(Into::into)
    }
}

pub struct SagaDriver {
    kind: &'static str,
    registry: Arc<SagaRegistry>,
    journal: Arc<dyn SagaJournal>,
}

impl SagaDriver {
    /// Sub-phase A/B-compatible constructor: in-memory journal, fresh
    /// registry. Tests and dev daemon use this.
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            registry: Arc::new(SagaRegistry::new()),
            journal: Arc::new(InMemorySagaJournal),
        }
    }

    /// Sub-phase C constructor: caller injects the registry + journal so
    /// admin-plane services share state with the driver.
    pub fn with_registry(
        kind: &'static str,
        registry: Arc<SagaRegistry>,
        journal: Arc<dyn SagaJournal>,
    ) -> Self {
        Self {
            kind,
            registry,
            journal,
        }
    }

    /// The kind tag passed to `new`, used for deadline error labelling.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Expose the registry handle so admin-plane services can read it.
    pub fn registry(&self) -> &Arc<SagaRegistry> {
        &self.registry
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    async fn persist(&self, saga_id: &str, event: SagaEvent) {
        if let Err(e) = self.journal.append_event(saga_id, event).await {
            tracing::warn!(
                saga = %self.kind,
                saga_id,
                error = %e,
                "saga journal append failed (best-effort, ignored)",
            );
        }
    }

    /// Run a saga: forward all steps; on first error, compensate prior
    /// steps in reverse and return the original forward error.
    pub async fn run(
        &self,
        ctx: SagaCtx,
        mut steps: Vec<Box<dyn SagaStep>>,
    ) -> Result<(), SagaError> {
        let mut completed: Vec<Box<dyn SagaStep>> = Vec::new();
        let kind = self.kind;

        // Open the in-memory record + persist the start event.
        self.registry.open(&ctx.saga_id, kind, &ctx.sid);
        self.persist(
            &ctx.saga_id,
            SagaEvent {
                event_type: SagaEventType::Started,
                saga_kind: kind.to_string(),
                sid: ctx.sid.value.clone(),
                step: None,
                seq: 0,
                timestamp_ms: Self::now_ms(),
            },
        )
        .await;

        for (i, step) in steps.drain(..).enumerate() {
            let name = step.name();
            self.registry.step_started(&ctx.saga_id, name);
            tracing::info!(
                saga = %kind,
                saga_id = %ctx.saga_id,
                step = name,
                idx = i,
                "saga forward",
            );
            match step.forward(&ctx).await {
                Ok(()) => {
                    self.registry.step_completed(&ctx.saga_id, name);
                    self.persist(
                        &ctx.saga_id,
                        SagaEvent {
                            event_type: SagaEventType::StepForward,
                            saga_kind: kind.to_string(),
                            sid: ctx.sid.value.clone(),
                            step: Some(name.to_string()),
                            seq: (i as u32) + 1,
                            timestamp_ms: Self::now_ms(),
                        },
                    )
                    .await;
                    completed.push(step);
                }
                Err(err) => {
                    tracing::warn!(
                        saga = %kind,
                        step = name,
                        error = %err,
                        "saga step failed; compensating",
                    );
                    let mut comp_seq = (i as u32) + 1;
                    while let Some(prev) = completed.pop() {
                        comp_seq += 1;
                        let prev_name = prev.name();
                        match prev.compensate(&ctx).await {
                            Ok(()) => {
                                self.registry.compensation_applied(&ctx.saga_id, prev_name);
                                self.persist(
                                    &ctx.saga_id,
                                    SagaEvent {
                                        event_type: SagaEventType::StepCompensated,
                                        saga_kind: kind.to_string(),
                                        sid: ctx.sid.value.clone(),
                                        step: Some(prev_name.to_string()),
                                        seq: comp_seq,
                                        timestamp_ms: Self::now_ms(),
                                    },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!(
                                    saga = %kind,
                                    step = prev_name,
                                    error = %e,
                                    "compensation failed (logged, NOT retried — see Spec C₂ §4.1)",
                                );
                            }
                        }
                    }
                    self.registry.close(&ctx.saga_id, SagaStatus::Compensated);
                    self.persist(
                        &ctx.saga_id,
                        SagaEvent {
                            event_type: SagaEventType::Failed,
                            saga_kind: kind.to_string(),
                            sid: ctx.sid.value.clone(),
                            step: Some(name.to_string()),
                            seq: comp_seq + 1,
                            timestamp_ms: Self::now_ms(),
                        },
                    )
                    .await;
                    return Err(err);
                }
            }
        }
        self.registry.close(&ctx.saga_id, SagaStatus::Succeeded);
        self.persist(
            &ctx.saga_id,
            SagaEvent {
                event_type: SagaEventType::Completed,
                saga_kind: kind.to_string(),
                sid: ctx.sid.value.clone(),
                step: None,
                seq: 0xffff_ffff,
                timestamp_ms: Self::now_ms(),
            },
        )
        .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FailOnForward {
        forwards: Arc<AtomicUsize>,
        compensates: Arc<AtomicUsize>,
        name: &'static str,
    }

    #[async_trait]
    impl SagaStep for FailOnForward {
        async fn forward(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
            self.forwards.fetch_add(1, Ordering::SeqCst);
            Err(SagaError::Forward {
                name: self.name,
                source: tonic::Status::internal("simulated"),
            })
        }
        async fn compensate(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
            self.compensates.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn name(&self) -> &'static str {
            self.name
        }
    }

    struct OkStep {
        forwards: Arc<AtomicUsize>,
        compensates: Arc<AtomicUsize>,
        name: &'static str,
    }

    #[async_trait]
    impl SagaStep for OkStep {
        async fn forward(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
            self.forwards.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn compensate(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
            self.compensates.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn name(&self) -> &'static str {
            self.name
        }
    }

    fn ctx() -> SagaCtx {
        SagaCtx {
            saga_id: "s-1".to_string(),
            user_id: "alice".to_string(),
            project_id: "p".to_string(),
            sid: aios_v1::SessionId {
                value: "sid".to_string(),
            },
            idempotency_key: None,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
            trace: tracing::Span::none(),
            claims: CapabilityClaims::default(),
        }
    }

    #[tokio::test]
    async fn forward_succeeds_when_all_steps_ok() {
        let f = Arc::new(AtomicUsize::new(0));
        let c = Arc::new(AtomicUsize::new(0));
        let driver = SagaDriver::new("test");
        let steps: Vec<Box<dyn SagaStep>> = vec![
            Box::new(OkStep {
                forwards: f.clone(),
                compensates: c.clone(),
                name: "a",
            }),
            Box::new(OkStep {
                forwards: f.clone(),
                compensates: c.clone(),
                name: "b",
            }),
        ];
        driver.run(ctx(), steps).await.expect("ok");
        assert_eq!(f.load(Ordering::SeqCst), 2);
        assert_eq!(c.load(Ordering::SeqCst), 0, "no compensation on success");
    }

    #[tokio::test]
    async fn compensations_run_in_reverse_on_failure() {
        let f = Arc::new(AtomicUsize::new(0));
        let c = Arc::new(AtomicUsize::new(0));
        let driver = SagaDriver::new("test");
        let steps: Vec<Box<dyn SagaStep>> = vec![
            Box::new(OkStep {
                forwards: f.clone(),
                compensates: c.clone(),
                name: "a",
            }),
            Box::new(OkStep {
                forwards: f.clone(),
                compensates: c.clone(),
                name: "b",
            }),
            Box::new(FailOnForward {
                forwards: f.clone(),
                compensates: c.clone(),
                name: "c",
            }),
        ];
        let err = driver.run(ctx(), steps).await.expect_err("must err");
        assert!(matches!(err, SagaError::Forward { .. }));
        // a forwarded + b forwarded + c forward (failed) = 3.
        assert_eq!(f.load(Ordering::SeqCst), 3);
        // a + b compensated; c never compensated because its forward failed.
        assert_eq!(c.load(Ordering::SeqCst), 2);
    }
}
