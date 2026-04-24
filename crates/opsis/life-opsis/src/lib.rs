//! Public surface for the Life Opsis world-state subsystem.
//! Use `--features schema` for DTO-only consumption without pulling in runtime deps.

#![forbid(unsafe_code)]

pub use opsis_core as core;
pub use opsis_engine as engine;
pub use opsis_lago as lago;

#[cfg(feature = "schema")]
pub use opsis_api_schema as schema;
