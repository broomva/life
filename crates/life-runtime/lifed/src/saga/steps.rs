//! Saga step impls for `Agent.CreateSession` per Spec C₂ §4.2.
//!
//! Each forward calls one substrate RPC; each compensate undoes it.
//! Implementations stay <20 LOC of body each — anything larger means
//! business logic is leaking into lifed.

use std::sync::Arc;

use async_trait::async_trait;

use crate::auth::keystore::Keystore;
use crate::auth::substrate_token::{Audience, mint_substrate_token};
use crate::saga::driver::{SagaCtx, SagaError, SagaStep};

use anima_proxy::AnimaCall;
use arcan_proxy::ArcanCall;
use haima_proxy::HaimaCall;
use lago_proxy::LagoCall;

pub struct CreateAgent {
    pub arcan: Arc<dyn ArcanCall>,
    pub ks: Arc<Keystore>,
}

#[async_trait]
impl SagaStep for CreateAgent {
    fn name(&self) -> &'static str {
        "create_agent"
    }
    async fn forward(&self, ctx: &SagaCtx) -> Result<(), SagaError> {
        let _token = mint_substrate_token(&ctx.claims, Audience::Arcan, &self.ks).map_err(|e| {
            SagaError::Forward {
                name: self.name(),
                source: tonic::Status::unauthenticated(e.to_string()),
            }
        })?;
        self.arcan
            .create_agent(&ctx.sid.value)
            .await
            .map(|_| ())
            .map_err(|e| SagaError::Forward {
                name: self.name(),
                source: e.into(),
            })
    }
    async fn compensate(&self, ctx: &SagaCtx) -> Result<(), SagaError> {
        self.arcan
            .destroy_agent(&ctx.sid.value)
            .await
            .map_err(|e| SagaError::Compensate {
                name: self.name(),
                source: e.into(),
            })
    }
}

pub struct OpenLagoNamespace {
    pub lago: Arc<dyn LagoCall>,
    pub ks: Arc<Keystore>,
}

#[async_trait]
impl SagaStep for OpenLagoNamespace {
    fn name(&self) -> &'static str {
        "open_lago_namespace"
    }
    async fn forward(&self, ctx: &SagaCtx) -> Result<(), SagaError> {
        let _token = mint_substrate_token(&ctx.claims, Audience::Lago, &self.ks).map_err(|e| {
            SagaError::Forward {
                name: self.name(),
                source: tonic::Status::unauthenticated(e.to_string()),
            }
        })?;
        self.lago
            .open_namespace(&ctx.sid.value)
            .await
            .map(|_| ())
            .map_err(|e| SagaError::Forward {
                name: self.name(),
                source: e.into(),
            })
    }
    async fn compensate(&self, ctx: &SagaCtx) -> Result<(), SagaError> {
        self.lago
            .close_namespace(&format!("session/{}", ctx.sid.value))
            .await
            .map_err(|e| SagaError::Compensate {
                name: self.name(),
                source: e.into(),
            })
    }
}

pub struct BindWallet {
    pub haima: Arc<dyn HaimaCall>,
    pub ks: Arc<Keystore>,
}

#[async_trait]
impl SagaStep for BindWallet {
    fn name(&self) -> &'static str {
        "bind_wallet"
    }
    async fn forward(&self, ctx: &SagaCtx) -> Result<(), SagaError> {
        let _token = mint_substrate_token(&ctx.claims, Audience::Haima, &self.ks).map_err(|e| {
            SagaError::Forward {
                name: self.name(),
                source: tonic::Status::unauthenticated(e.to_string()),
            }
        })?;
        self.haima
            .bind_wallet(&ctx.sid.value, &ctx.project_id)
            .await
            .map(|_| ())
            .map_err(|e| SagaError::Forward {
                name: self.name(),
                source: e.into(),
            })
    }
    async fn compensate(&self, ctx: &SagaCtx) -> Result<(), SagaError> {
        self.haima
            .unbind_wallet(&format!("wallet-{}", ctx.sid.value))
            .await
            .map_err(|e| SagaError::Compensate {
                name: self.name(),
                source: e.into(),
            })
    }
}

pub struct RegisterAnimaSession {
    pub anima: Arc<dyn AnimaCall>,
    pub ks: Arc<Keystore>,
}

#[async_trait]
impl SagaStep for RegisterAnimaSession {
    fn name(&self) -> &'static str {
        "register_anima_session"
    }
    async fn forward(&self, ctx: &SagaCtx) -> Result<(), SagaError> {
        let _token = mint_substrate_token(&ctx.claims, Audience::Anima, &self.ks).map_err(|e| {
            SagaError::Forward {
                name: self.name(),
                source: tonic::Status::unauthenticated(e.to_string()),
            }
        })?;
        self.anima
            .register_session(&ctx.sid.value, &ctx.user_id)
            .await
            .map_err(|e| SagaError::Forward {
                name: self.name(),
                source: e.into(),
            })
    }
    async fn compensate(&self, ctx: &SagaCtx) -> Result<(), SagaError> {
        self.anima
            .mark_session_closed(&ctx.sid.value)
            .await
            .map_err(|e| SagaError::Compensate {
                name: self.name(),
                source: e.into(),
            })
    }
}
