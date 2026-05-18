//! Auxiliary services mounted alongside the public-plane proxy.
//!
//! Sub-phase A ships only `/healthz`.
//! Sub-phase C adds `ws` (WebSocket bidi pump for `Agent.StreamSession`).
//! Sub-phase D adds `rate_limit` (token-bucket limiter), the admin
//! plane UDS, plus a `cert_watch` reloader.

pub mod agent_http;
pub mod anima_custody;
pub mod anthropic_messages;
pub mod cert_watch;
pub mod health;
pub mod rate_limit;
pub mod ws;
