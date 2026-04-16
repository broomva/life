//! Contact information produced by collision detection.

use crate::math::Vec2;

/// A single contact point in a collision manifold.
#[derive(Debug, Clone, Copy)]
pub struct ContactPoint {
    /// World-space position of the contact.
    pub position: Vec2,
    /// Penetration depth (positive means overlapping).
    pub penetration: f64,
}

/// A contact manifold between two bodies.
///
/// Contains one or more contact points sharing the same contact normal.
/// The normal always points from `body_a` toward `body_b`.
#[derive(Debug, Clone)]
pub struct ContactManifold {
    /// Index of the first body.
    pub body_a: usize,
    /// Index of the second body.
    pub body_b: usize,
    /// Contact normal (unit vector from body A toward body B).
    pub normal: Vec2,
    /// Contact points.
    pub contacts: Vec<ContactPoint>,
}
