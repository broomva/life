//! Saga step types for `Agent.CreateSession`. Sub-phase A bodies are stubs.

use async_trait::async_trait;

use crate::saga::driver::{SagaCtx, SagaError, SagaStep};

pub struct CreateAgent;
pub struct OpenLagoNamespace;
pub struct BindWallet;
pub struct RegisterAnimaSession;

#[async_trait]
impl SagaStep for CreateAgent {
    async fn forward(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
        Ok(())
    }
    async fn compensate(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
        Ok(())
    }
    fn name(&self) -> &'static str {
        "create_agent"
    }
}

#[async_trait]
impl SagaStep for OpenLagoNamespace {
    async fn forward(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
        Ok(())
    }
    async fn compensate(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
        Ok(())
    }
    fn name(&self) -> &'static str {
        "open_lago_namespace"
    }
}

#[async_trait]
impl SagaStep for BindWallet {
    async fn forward(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
        Ok(())
    }
    async fn compensate(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
        Ok(())
    }
    fn name(&self) -> &'static str {
        "bind_wallet"
    }
}

#[async_trait]
impl SagaStep for RegisterAnimaSession {
    async fn forward(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
        Ok(())
    }
    async fn compensate(&self, _ctx: &SagaCtx) -> Result<(), SagaError> {
        Ok(())
    }
    fn name(&self) -> &'static str {
        "register_anima_session"
    }
}
