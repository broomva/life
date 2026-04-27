//! Saga driver per Spec C₂ §4.1.
//!
//! Runs `Vec<Box<dyn SagaStep>>` left-to-right; on first step error,
//! invokes `compensate` on each previously-completed step in reverse order.
//! Compensation failures are logged but not retried (per spec, the saga
//! must not loop on compensation pathologies — operators force-recover via
//! the admin plane in sub-phase C).

use std::time::Instant;

use async_trait::async_trait;
use thiserror::Error;

use aios_proto::aios::v1 as aios_v1;

use crate::auth::capability::CapabilityClaims;

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

pub struct SagaDriver {
    kind: &'static str,
}

impl SagaDriver {
    pub fn new(kind: &'static str) -> Self {
        Self { kind }
    }

    /// The kind tag passed to `new`, used for deadline error labelling.
    pub fn kind(&self) -> &'static str {
        self.kind
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
        for (i, step) in steps.drain(..).enumerate() {
            let name = step.name();
            tracing::info!(
                saga = %kind,
                saga_id = %ctx.saga_id,
                step = name,
                idx = i,
                "saga forward",
            );
            match step.forward(&ctx).await {
                Ok(()) => completed.push(step),
                Err(err) => {
                    tracing::warn!(
                        saga = %kind,
                        step = name,
                        error = %err,
                        "saga step failed; compensating",
                    );
                    while let Some(prev) = completed.pop() {
                        let prev_name = prev.name();
                        if let Err(e) = prev.compensate(&ctx).await {
                            tracing::error!(
                                saga = %kind,
                                step = prev_name,
                                error = %e,
                                "compensation failed (logged, NOT retried — see Spec C₂ §4.1)",
                            );
                        }
                    }
                    return Err(err);
                }
            }
        }
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
