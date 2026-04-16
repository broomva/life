//! `physics-engine` — 2D rigid body physics simulation engine.
//!
//! Provides a complete physics pipeline:
//! - **Broad phase**: AABB-based pair culling
//! - **Narrow phase**: SAT collision detection (circle-circle, circle-polygon, polygon-polygon)
//! - **Solver**: Impulse-based collision resolution with friction
//! - **Integrator**: Semi-implicit Euler integration
//! - **World**: Top-level simulation container

pub mod broadphase;
pub mod integrator;
pub mod narrowphase;
pub mod solver;
pub mod world;

pub use world::{PhysicsWorld, WorldConfig};
