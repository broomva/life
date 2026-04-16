//! `physics-core` — Core types for the 2D rigid body physics engine.
//!
//! This crate contains **zero IO** — only types, traits, and pure logic.
//! Provides math primitives, shape definitions, rigid body representation,
//! contact manifolds, and material properties for physics simulation.

pub mod body;
pub mod contact;
pub mod error;
pub mod material;
pub mod math;
pub mod shape;

pub use body::{BodyHandle, BodyKind, RigidBody};
pub use contact::{ContactManifold, ContactPoint};
pub use error::{PhysicsError, PhysicsResult};
pub use material::Material;
pub use math::{Aabb, Vec2};
pub use shape::Shape;
