//! arcan-proxy — typed tonic client for the arcan substrate.

#![cfg_attr(not(test), deny(unsafe_code))]

pub mod anthropic;
pub mod client;
pub mod conversions;
pub mod error;
pub mod grounding;
pub mod vercel_ai_gateway;

pub use anthropic::{
    AnthropicArcan, AnthropicArcanConfig, DEFAULT_BASE_URL as ANTHROPIC_DEFAULT_BASE_URL,
    DEFAULT_MODEL as ANTHROPIC_DEFAULT_MODEL,
};
pub use client::{ArcanCall, ArcanProxy, PoolGuardedStream, Pooled};
pub use error::{ArcanProxyError, ArcanProxyResult, RetryClass};
pub use grounding::{DEFAULT_CHAT_SYSTEM_PROMPT, SYSTEM_PROMPT_ENV, resolve_system_prompt};
pub use vercel_ai_gateway::{
    DEFAULT_BASE_URL as VERCEL_AI_GATEWAY_DEFAULT_BASE_URL,
    DEFAULT_MODEL as VERCEL_AI_GATEWAY_DEFAULT_MODEL, VercelAiGatewayArcan, VercelAiGatewayConfig,
};
