//! Auxiliary services mounted alongside the public-plane proxy.
//!
//! Sub-phase A ships only `/healthz`.
//! Sub-phase C adds `ws` (WebSocket bidi pump for `Agent.StreamSession`).
//! Sub-phase D adds `/readyz`, `/version`, `/metrics`, and the admin-
//! plane UDS listener.

pub mod health;
pub mod ws;
