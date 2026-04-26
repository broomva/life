//! No-op saga driver placeholder. The real driver lands in B6.

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

    /// Sub-phase A no-op. B6 replaces with the real driver.
    pub async fn run(
        &self,
        _ctx: SagaCtx,
        _steps: Vec<Box<dyn SagaStep>>,
    ) -> Result<(), SagaError> {
        Ok(())
    }
}
