//! Impulse-based collision solver with friction and position correction.
//!
//! Resolves collisions by applying impulses that modify body velocities
//! so they separate (or stop penetrating) in the next integration step.
//! Uses Baumgarte stabilization for positional correction to prevent sinking.

use physics_core::{BodyKind, ContactManifold, RigidBody, Vec2};

/// Solver configuration.
#[derive(Debug, Clone, Copy)]
pub struct SolverConfig {
    /// Number of velocity solver iterations per step.
    pub velocity_iterations: usize,
    /// Baumgarte stabilization factor (fraction of penetration corrected per step).
    pub position_correction_percent: f64,
    /// Penetration slop — small amount of overlap allowed to prevent jitter.
    pub position_correction_slop: f64,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            velocity_iterations: 8,
            position_correction_percent: 0.2,
            position_correction_slop: 0.01,
        }
    }
}

/// Resolves all contacts by iteratively applying impulses.
pub fn solve(bodies: &mut [RigidBody], manifolds: &[ContactManifold], config: &SolverConfig) {
    // Iterative impulse solver — multiple passes improve stacking stability
    for _ in 0..config.velocity_iterations {
        for manifold in manifolds {
            solve_manifold_velocities(bodies, manifold);
        }
    }

    // Positional correction (Baumgarte stabilization)
    for manifold in manifolds {
        correct_positions(bodies, manifold, config);
    }
}

/// Applies impulses for a single contact manifold.
fn solve_manifold_velocities(bodies: &mut [RigidBody], manifold: &ContactManifold) {
    let a = manifold.body_a;
    let b = manifold.body_b;
    let normal = manifold.normal;

    for contact in &manifold.contacts {
        // Read needed values
        let inv_mass_a = bodies[a].inv_mass;
        let inv_mass_b = bodies[b].inv_mass;
        let inv_inertia_a = bodies[a].inv_inertia;
        let inv_inertia_b = bodies[b].inv_inertia;
        let pos_a = bodies[a].position;
        let pos_b = bodies[b].position;
        let vel_a = bodies[a].velocity;
        let vel_b = bodies[b].velocity;
        let ang_vel_a = bodies[a].angular_velocity;
        let ang_vel_b = bodies[b].angular_velocity;

        let r_a = contact.position - pos_a;
        let r_b = contact.position - pos_b;

        // Relative velocity at the contact point
        let rel_vel = (vel_b + Vec2::cross_scalar(ang_vel_b, r_b))
            - (vel_a + Vec2::cross_scalar(ang_vel_a, r_a));

        let vel_along_normal = rel_vel.dot(normal);

        // Bodies are already separating
        if vel_along_normal > 0.0 {
            continue;
        }

        // Compute restitution (use minimum)
        let e = bodies[a]
            .material
            .restitution
            .min(bodies[b].material.restitution);

        // Effective mass for the normal direction
        let ra_cross_n = r_a.cross(normal);
        let rb_cross_n = r_b.cross(normal);
        let inv_mass_sum = inv_mass_a
            + inv_mass_b
            + ra_cross_n * ra_cross_n * inv_inertia_a
            + rb_cross_n * rb_cross_n * inv_inertia_b;

        if inv_mass_sum.abs() < f64::EPSILON {
            continue;
        }

        // Normal impulse magnitude
        let j = -(1.0 + e) * vel_along_normal / inv_mass_sum;
        let impulse = normal * j;

        // Apply normal impulse
        bodies[a].velocity -= impulse * inv_mass_a;
        bodies[b].velocity += impulse * inv_mass_b;
        bodies[a].angular_velocity -= r_a.cross(impulse) * inv_inertia_a;
        bodies[b].angular_velocity += r_b.cross(impulse) * inv_inertia_b;

        // ── Friction impulse ──────────────────────────────────────────

        // Recompute relative velocity after normal impulse
        let vel_a = bodies[a].velocity;
        let vel_b = bodies[b].velocity;
        let ang_vel_a = bodies[a].angular_velocity;
        let ang_vel_b = bodies[b].angular_velocity;

        let rel_vel = (vel_b + Vec2::cross_scalar(ang_vel_b, r_b))
            - (vel_a + Vec2::cross_scalar(ang_vel_a, r_a));

        // Tangent direction
        let tangent_vel = rel_vel - normal * rel_vel.dot(normal);
        let tangent = tangent_vel.normalized();
        if tangent.magnitude_squared() < f64::EPSILON {
            continue;
        }

        // Friction impulse magnitude
        let jt = -rel_vel.dot(tangent) / inv_mass_sum;

        // Coulomb's friction model
        let mu_s = (bodies[a].material.static_friction + bodies[b].material.static_friction) * 0.5;
        let mu_d =
            (bodies[a].material.dynamic_friction + bodies[b].material.dynamic_friction) * 0.5;

        let friction_impulse = if jt.abs() < j * mu_s {
            tangent * jt
        } else {
            tangent * (-j * mu_d)
        };

        bodies[a].velocity -= friction_impulse * inv_mass_a;
        bodies[b].velocity += friction_impulse * inv_mass_b;
        bodies[a].angular_velocity -= r_a.cross(friction_impulse) * inv_inertia_a;
        bodies[b].angular_velocity += r_b.cross(friction_impulse) * inv_inertia_b;
    }
}

/// Baumgarte positional correction to prevent sinking.
fn correct_positions(bodies: &mut [RigidBody], manifold: &ContactManifold, config: &SolverConfig) {
    let a = manifold.body_a;
    let b = manifold.body_b;
    let inv_mass_a = bodies[a].inv_mass;
    let inv_mass_b = bodies[b].inv_mass;
    let inv_mass_sum = inv_mass_a + inv_mass_b;

    if inv_mass_sum.abs() < f64::EPSILON {
        return;
    }

    for contact in &manifold.contacts {
        let correction_magnitude = (contact.penetration - config.position_correction_slop).max(0.0)
            * config.position_correction_percent
            / inv_mass_sum;

        let correction = manifold.normal * correction_magnitude;

        if bodies[a].kind != BodyKind::Static {
            bodies[a].position -= correction * inv_mass_a;
        }
        if bodies[b].kind != BodyKind::Static {
            bodies[b].position += correction * inv_mass_b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use physics_core::{ContactPoint, Material};

    #[test]
    fn separating_bodies_not_resolved() {
        let mut bodies = vec![
            RigidBody::dynamic(physics_core::Shape::circle(1.0), Material::default())
                .with_position(Vec2::new(0.0, 0.0)),
            RigidBody::dynamic(physics_core::Shape::circle(1.0), Material::default())
                .with_position(Vec2::new(2.0, 0.0)),
        ];

        // Bodies already moving apart
        bodies[0].velocity = Vec2::new(-1.0, 0.0);
        bodies[1].velocity = Vec2::new(1.0, 0.0);

        let manifold = ContactManifold {
            body_a: 0,
            body_b: 1,
            normal: Vec2::RIGHT,
            contacts: vec![ContactPoint {
                position: Vec2::new(1.0, 0.0),
                penetration: 0.1,
            }],
        };

        let vel_a_before = bodies[0].velocity;
        let vel_b_before = bodies[1].velocity;

        solve(&mut bodies, &[manifold], &SolverConfig::default());

        // Velocities should not change (separating)
        assert_eq!(bodies[0].velocity, vel_a_before);
        assert_eq!(bodies[1].velocity, vel_b_before);
    }

    #[test]
    fn approaching_bodies_get_impulse() {
        let mut bodies = vec![
            RigidBody::dynamic(physics_core::Shape::circle(1.0), Material::default())
                .with_position(Vec2::new(0.0, 0.0))
                .with_velocity(Vec2::new(5.0, 0.0)),
            RigidBody::dynamic(physics_core::Shape::circle(1.0), Material::default())
                .with_position(Vec2::new(1.5, 0.0))
                .with_velocity(Vec2::new(-5.0, 0.0)),
        ];

        let manifold = ContactManifold {
            body_a: 0,
            body_b: 1,
            normal: Vec2::RIGHT,
            contacts: vec![ContactPoint {
                position: Vec2::new(0.75, 0.0),
                penetration: 0.5,
            }],
        };

        solve(&mut bodies, &[manifold], &SolverConfig::default());

        // After resolution, body A should be moving left (bounced)
        assert!(bodies[0].velocity.x < 5.0);
        // Body B should be moving right (bounced)
        assert!(bodies[1].velocity.x > -5.0);
    }
}
