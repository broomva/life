//! Facade crate for the Life physics engine.
//!
//! Re-exports `physics-core` (types) and `physics-engine` (simulation)
//! under a single convenient namespace.

pub use physics_core as core;
pub use physics_engine as engine;
