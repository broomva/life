//! Rigid body representation.

use serde::{Deserialize, Serialize};

use crate::material::Material;
use crate::math::Vec2;
use crate::shape::Shape;

/// Opaque handle to a body in the physics world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BodyHandle(pub u32);

impl BodyHandle {
    /// Returns the raw index (useful for indexing into the body array).
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Whether the body is static, dynamic, or kinematic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyKind {
    /// Immovable body with infinite mass (e.g., ground, walls).
    Static,
    /// Fully simulated body affected by forces and collisions.
    Dynamic,
    /// User-controlled body that moves at a set velocity but isn't affected by forces.
    Kinematic,
}

/// A 2D rigid body with position, velocity, mass, shape, and material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigidBody {
    // ── Transform ──
    pub position: Vec2,
    pub angle: f64,

    // ── Velocity ──
    pub velocity: Vec2,
    pub angular_velocity: f64,

    // ── Accumulated forces (cleared each step) ──
    pub force: Vec2,
    pub torque: f64,

    // ── Mass properties ──
    pub mass: f64,
    pub inv_mass: f64,
    pub inertia: f64,
    pub inv_inertia: f64,

    // ── Shape & material ──
    pub shape: Shape,
    pub material: Material,
    pub kind: BodyKind,
}

impl RigidBody {
    /// Creates a new dynamic body from a shape and material.
    /// Mass is computed from the shape area and material density.
    pub fn dynamic(shape: Shape, material: Material) -> Self {
        let area = shape.area();
        let mass = area * material.density;
        let inertia = shape.moment_of_inertia(mass);
        Self {
            position: Vec2::ZERO,
            angle: 0.0,
            velocity: Vec2::ZERO,
            angular_velocity: 0.0,
            force: Vec2::ZERO,
            torque: 0.0,
            mass,
            inv_mass: 1.0 / mass,
            inertia,
            inv_inertia: 1.0 / inertia,
            shape,
            material,
            kind: BodyKind::Dynamic,
        }
    }

    /// Creates a static (immovable) body.
    pub fn stationary(shape: Shape, material: Material) -> Self {
        Self {
            position: Vec2::ZERO,
            angle: 0.0,
            velocity: Vec2::ZERO,
            angular_velocity: 0.0,
            force: Vec2::ZERO,
            torque: 0.0,
            mass: 0.0,
            inv_mass: 0.0,
            inertia: 0.0,
            inv_inertia: 0.0,
            shape,
            material,
            kind: BodyKind::Static,
        }
    }

    /// Creates a kinematic body (user-driven, not affected by forces).
    pub fn kinematic(shape: Shape, material: Material) -> Self {
        Self {
            position: Vec2::ZERO,
            angle: 0.0,
            velocity: Vec2::ZERO,
            angular_velocity: 0.0,
            force: Vec2::ZERO,
            torque: 0.0,
            mass: 0.0,
            inv_mass: 0.0,
            inertia: 0.0,
            inv_inertia: 0.0,
            shape,
            material,
            kind: BodyKind::Kinematic,
        }
    }

    /// Builder: set the position.
    pub fn with_position(mut self, position: Vec2) -> Self {
        self.position = position;
        self
    }

    /// Builder: set the angle (radians).
    pub fn with_angle(mut self, angle: f64) -> Self {
        self.angle = angle;
        self
    }

    /// Builder: set the initial velocity.
    pub fn with_velocity(mut self, velocity: Vec2) -> Self {
        self.velocity = velocity;
        self
    }

    /// Apply a force at the center of mass (accumulated until the next step).
    pub fn apply_force(&mut self, force: Vec2) {
        self.force += force;
    }

    /// Apply a force at a world-space point (generates torque).
    pub fn apply_force_at(&mut self, force: Vec2, point: Vec2) {
        self.force += force;
        let r = point - self.position;
        self.torque += r.cross(force);
    }

    /// Apply an instantaneous impulse at the center of mass.
    pub fn apply_impulse(&mut self, impulse: Vec2) {
        self.velocity += impulse * self.inv_mass;
    }

    /// Apply an instantaneous impulse at a world-space point.
    pub fn apply_impulse_at(&mut self, impulse: Vec2, point: Vec2) {
        self.velocity += impulse * self.inv_mass;
        let r = point - self.position;
        self.angular_velocity += r.cross(impulse) * self.inv_inertia;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_body_has_mass() {
        let body = RigidBody::dynamic(Shape::circle(1.0), Material::default());
        assert!(body.mass > 0.0);
        assert!(body.inv_mass > 0.0);
        assert!(body.inertia > 0.0);
        assert_eq!(body.kind, BodyKind::Dynamic);
    }

    #[test]
    fn static_body_has_zero_inv_mass() {
        let body = RigidBody::stationary(Shape::circle(1.0), Material::default());
        assert_eq!(body.mass, 0.0);
        assert_eq!(body.inv_mass, 0.0);
        assert_eq!(body.inv_inertia, 0.0);
        assert_eq!(body.kind, BodyKind::Static);
    }

    #[test]
    fn builder_pattern() {
        let body = RigidBody::dynamic(Shape::circle(1.0), Material::default())
            .with_position(Vec2::new(5.0, 10.0))
            .with_velocity(Vec2::new(1.0, 0.0))
            .with_angle(0.5);
        assert_eq!(body.position, Vec2::new(5.0, 10.0));
        assert_eq!(body.velocity, Vec2::new(1.0, 0.0));
        assert_eq!(body.angle, 0.5);
    }

    #[test]
    fn apply_force_accumulates() {
        let mut body = RigidBody::dynamic(Shape::circle(1.0), Material::default());
        body.apply_force(Vec2::new(10.0, 0.0));
        body.apply_force(Vec2::new(0.0, 5.0));
        assert_eq!(body.force, Vec2::new(10.0, 5.0));
    }

    #[test]
    fn apply_impulse_changes_velocity() {
        let mut body = RigidBody::dynamic(Shape::circle(1.0), Material::default());
        let inv_mass = body.inv_mass;
        body.apply_impulse(Vec2::new(10.0, 0.0));
        assert_eq!(body.velocity, Vec2::new(10.0 * inv_mass, 0.0));
    }

    #[test]
    fn body_handle_index() {
        let h = BodyHandle(42);
        assert_eq!(h.index(), 42);
    }
}
