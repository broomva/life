//! Semi-implicit Euler integration.
//!
//! Integrates velocity from forces, then position from velocity.
//! This ordering (velocity-first) provides better energy conservation
//! than explicit Euler at the same computational cost.

use physics_core::{BodyKind, RigidBody, Vec2};

/// Applies accumulated forces to update velocities (for dynamic bodies only).
pub fn integrate_velocities(bodies: &mut [RigidBody], dt: f64) {
    for body in bodies.iter_mut() {
        if body.kind != BodyKind::Dynamic {
            continue;
        }
        body.velocity += (body.force * body.inv_mass) * dt;
        body.angular_velocity += body.torque * body.inv_inertia * dt;
    }
}

/// Updates positions from velocities (for dynamic and kinematic bodies).
pub fn integrate_positions(bodies: &mut [RigidBody], dt: f64) {
    for body in bodies.iter_mut() {
        if body.kind == BodyKind::Static {
            continue;
        }
        body.position += body.velocity * dt;
        body.angle += body.angular_velocity * dt;
    }
}

/// Clears accumulated forces and torques on all bodies.
pub fn clear_forces(bodies: &mut [RigidBody]) {
    for body in bodies.iter_mut() {
        body.force = Vec2::ZERO;
        body.torque = 0.0;
    }
}

/// Applies a uniform gravity force to all dynamic bodies.
pub fn apply_gravity(bodies: &mut [RigidBody], gravity: Vec2) {
    for body in bodies.iter_mut() {
        if body.kind == BodyKind::Dynamic {
            body.force += gravity * body.mass;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use physics_core::{Material, Shape};

    #[test]
    fn gravity_accelerates_dynamic_body() {
        let mut bodies = vec![RigidBody::dynamic(Shape::circle(1.0), Material::default())];

        apply_gravity(&mut bodies, Vec2::new(0.0, -9.81));
        integrate_velocities(&mut bodies, 1.0);

        assert!(bodies[0].velocity.y < 0.0);
    }

    #[test]
    fn static_body_unaffected_by_gravity() {
        let mut bodies = vec![RigidBody::stationary(
            Shape::circle(1.0),
            Material::default(),
        )];

        apply_gravity(&mut bodies, Vec2::new(0.0, -9.81));
        integrate_velocities(&mut bodies, 1.0);
        integrate_positions(&mut bodies, 1.0);

        assert_eq!(bodies[0].velocity, Vec2::ZERO);
        assert_eq!(bodies[0].position, Vec2::ZERO);
    }

    #[test]
    fn position_updates_from_velocity() {
        let mut bodies = vec![
            RigidBody::dynamic(Shape::circle(1.0), Material::default())
                .with_velocity(Vec2::new(10.0, 0.0)),
        ];

        integrate_positions(&mut bodies, 0.5);

        assert!((bodies[0].position.x - 5.0).abs() < 1e-10);
    }

    #[test]
    fn clear_forces_resets() {
        let mut bodies = vec![RigidBody::dynamic(Shape::circle(1.0), Material::default())];
        bodies[0].force = Vec2::new(100.0, 200.0);
        bodies[0].torque = 50.0;

        clear_forces(&mut bodies);

        assert_eq!(bodies[0].force, Vec2::ZERO);
        assert_eq!(bodies[0].torque, 0.0);
    }

    #[test]
    fn kinematic_body_moves_but_ignores_forces() {
        let mut bodies = vec![
            RigidBody::kinematic(Shape::circle(1.0), Material::default())
                .with_velocity(Vec2::new(5.0, 0.0)),
        ];
        bodies[0].force = Vec2::new(1000.0, 0.0);

        integrate_velocities(&mut bodies, 1.0);
        integrate_positions(&mut bodies, 1.0);

        // Velocity unchanged by force (kinematic)
        assert_eq!(bodies[0].velocity, Vec2::new(5.0, 0.0));
        // Position updated by velocity
        assert!((bodies[0].position.x - 5.0).abs() < 1e-10);
    }
}
