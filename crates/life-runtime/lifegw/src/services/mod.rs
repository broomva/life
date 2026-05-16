//! Auxiliary services mounted alongside the public-plane proxy.
//!
//! Sub-phase A ships only `/healthz`.
//! Sub-phase C adds `ws` (WebSocket bidi pump for `Agent.StreamSession`).
//! Sub-phase D adds `rate_limit` (token-bucket limiter), the admin
//! plane UDS, plus a `cert_watch` reloader.
//! Lumen Phase α₁ adds `auth_bootstrap` (`POST /life/v1/auth/bootstrap`
//! mints a Tier-User cap so `scripts/lumen-app.sh` can inject it via
//! `WARP_USER_SECRET` and bypass Firebase entirely).

pub mod agent_http;
pub mod anima_custody;
pub mod auth_bootstrap;
pub mod cert_watch;
pub mod health;
pub mod rate_limit;
pub mod ws;
