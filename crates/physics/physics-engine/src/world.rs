//! Top-level physics simulation container.
//!
//! [`PhysicsWorld`] manages bodies and runs the simulation step:
//! apply gravity → integrate velocities → detect collisions → solve → integrate positions.

use physics_core::{BodyHandle, ContactManifold, RigidBody, Vec2};

use crate::broadphase;
use crate::integrator;
use crate::narrowphase;
use crate::solver::{self, SolverConfig};

/// Configuration for the physics world.
#[derive(Debug, Clone, Copy)]
pub struct WorldConfig {
    /// Gravity acceleration vector (default: 9.81 m/s^2 downward).
    pub gravity: Vec2,
    /// Solver configuration.
    pub solver: SolverConfig,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            gravity: Vec2::new(0.0, -9.81),
            solver: SolverConfig::default(),
        }
    }
}

/// The physics simulation world.
///
/// Holds all rigid bodies and steps the simulation forward in time.
pub struct PhysicsWorld {
    bodies: Vec<RigidBody>,
    config: WorldConfig,
}

impl PhysicsWorld {
    /// Creates a new empty physics world with the given configuration.
    pub fn new(config: WorldConfig) -> Self {
        Self {
            bodies: Vec::new(),
            config,
        }
    }

    /// Creates a world with default configuration.
    pub fn with_default_config() -> Self {
        Self::new(WorldConfig::default())
    }

    /// Adds a body to the world and returns its handle.
    pub fn add_body(&mut self, body: RigidBody) -> BodyHandle {
        let idx = self.bodies.len() as u32;
        self.bodies.push(body);
        BodyHandle(idx)
    }

    /// Returns a reference to a body by handle.
    pub fn body(&self, handle: BodyHandle) -> &RigidBody {
        &self.bodies[handle.index()]
    }

    /// Returns a mutable reference to a body by handle.
    pub fn body_mut(&mut self, handle: BodyHandle) -> &mut RigidBody {
        &mut self.bodies[handle.index()]
    }

    /// Returns the number of bodies in the world.
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// Returns an iterator over all bodies.
    pub fn bodies(&self) -> &[RigidBody] {
        &self.bodies
    }

    /// Returns the world configuration.
    pub fn config(&self) -> &WorldConfig {
        &self.config
    }

    /// Sets the gravity vector.
    pub fn set_gravity(&mut self, gravity: Vec2) {
        self.config.gravity = gravity;
    }

    /// Advances the simulation by `dt` seconds.
    ///
    /// Pipeline:
    /// 1. Apply gravity to dynamic bodies
    /// 2. Integrate velocities (semi-implicit Euler)
    /// 3. Broad-phase collision detection (AABB overlap)
    /// 4. Narrow-phase collision detection (SAT / circle tests)
    /// 5. Solve contacts (impulse-based with friction)
    /// 6. Integrate positions
    /// 7. Clear accumulated forces
    pub fn step(&mut self, dt: f64) {
        // 1. Apply gravity
        integrator::apply_gravity(&mut self.bodies, self.config.gravity);

        // 2. Integrate velocities
        integrator::integrate_velocities(&mut self.bodies, dt);

        // 3. Broad phase
        let pairs = broadphase::detect_pairs(&self.bodies);

        // 4. Narrow phase
        let manifolds: Vec<ContactManifold> = pairs
            .iter()
            .filter_map(|pair| {
                let a = &self.bodies[pair.a];
                let b = &self.bodies[pair.b];
                narrowphase::detect(
                    pair.a, &a.shape, a.position, a.angle, pair.b, &b.shape, b.position, b.angle,
                )
            })
            .collect();

        // 5. Solve contacts
        solver::solve(&mut self.bodies, &manifolds, &self.config.solver);

        // 6. Integrate positions
        integrator::integrate_positions(&mut self.bodies, dt);

        // 7. Clear forces
        integrator::clear_forces(&mut self.bodies);
    }

    /// Applies a force to a body (accumulated until next step).
    pub fn apply_force(&mut self, handle: BodyHandle, force: Vec2) {
        self.bodies[handle.index()].apply_force(force);
    }

    /// Applies an instantaneous impulse to a body at its center of mass.
    pub fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec2) {
        self.bodies[handle.index()].apply_impulse(impulse);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use physics_core::{Material, Shape};

    #[test]
    fn empty_world_steps_without_error() {
        let mut world = PhysicsWorld::with_default_config();
        world.step(1.0 / 60.0);
        assert_eq!(world.body_count(), 0);
    }

    #[test]
    fn gravity_moves_body_downward() {
        let mut world = PhysicsWorld::with_default_config();
        let ball = RigidBody::dynamic(Shape::circle(0.5), Material::default())
            .with_position(Vec2::new(0.0, 10.0));
        let handle = world.add_body(ball);

        // Step 60 frames at 60 FPS (1 second)
        for _ in 0..60 {
            world.step(1.0 / 60.0);
        }

        // Ball should have fallen
        assert!(world.body(handle).position.y < 10.0);
        assert!(world.body(handle).velocity.y < 0.0);
    }

    #[test]
    fn static_body_stays_put() {
        let mut world = PhysicsWorld::with_default_config();
        let floor = RigidBody::stationary(Shape::rectangle(20.0, 1.0), Material::default())
            .with_position(Vec2::new(0.0, -5.0));
        let handle = world.add_body(floor);

        for _ in 0..60 {
            world.step(1.0 / 60.0);
        }

        assert_eq!(world.body(handle).position, Vec2::new(0.0, -5.0));
        assert_eq!(world.body(handle).velocity, Vec2::ZERO);
    }

    #[test]
    fn ball_bounces_off_floor() {
        let mut world = PhysicsWorld::new(WorldConfig {
            gravity: Vec2::new(0.0, -10.0),
            solver: SolverConfig {
                velocity_iterations: 10,
                ..SolverConfig::default()
            },
        });

        // Floor at y = 0 (static, 20 wide, 1 tall → top face at y = 0.5)
        let floor = RigidBody::stationary(
            Shape::rectangle(20.0, 1.0),
            Material {
                restitution: 1.0,
                ..Material::default()
            },
        )
        .with_position(Vec2::new(0.0, 0.0));

        // Ball above the floor
        let ball = RigidBody::dynamic(
            Shape::circle(0.5),
            Material {
                restitution: 1.0,
                ..Material::default()
            },
        )
        .with_position(Vec2::new(0.0, 5.0));

        world.add_body(floor);
        let ball_handle = world.add_body(ball);

        // Run simulation
        for _ in 0..300 {
            world.step(1.0 / 60.0);
        }

        // Ball should not have fallen through the floor
        let ball_y = world.body(ball_handle).position.y;
        assert!(ball_y > -1.0, "ball fell through floor: y = {ball_y}");
    }

    #[test]
    fn two_dynamic_bodies_collide() {
        let mut world = PhysicsWorld::new(WorldConfig {
            gravity: Vec2::ZERO, // no gravity
            ..WorldConfig::default()
        });

        let a = RigidBody::dynamic(Shape::circle(1.0), Material::default())
            .with_position(Vec2::new(-2.0, 0.0))
            .with_velocity(Vec2::new(5.0, 0.0));
        let b = RigidBody::dynamic(Shape::circle(1.0), Material::default())
            .with_position(Vec2::new(2.0, 0.0))
            .with_velocity(Vec2::new(-5.0, 0.0));

        let ha = world.add_body(a);
        let hb = world.add_body(b);

        for _ in 0..60 {
            world.step(1.0 / 60.0);
        }

        // After collision, bodies should have reversed (or slowed) their velocities
        // With default restitution 0.3, they won't fully reverse but should change direction
        let va = world.body(ha).velocity.x;
        let vb = world.body(hb).velocity.x;
        // Bodies should have exchanged some momentum
        assert!(va < 5.0, "body A should have slowed down: vx = {va}");
        assert!(vb > -5.0, "body B should have slowed down: vx = {vb}");
    }

    #[test]
    fn apply_force_accelerates_body() {
        let mut world = PhysicsWorld::new(WorldConfig {
            gravity: Vec2::ZERO,
            ..WorldConfig::default()
        });

        let body = RigidBody::dynamic(Shape::circle(1.0), Material::default());
        let handle = world.add_body(body);

        // Apply horizontal force for 1 second (60 steps)
        for _ in 0..60 {
            world.apply_force(handle, Vec2::new(100.0, 0.0));
            world.step(1.0 / 60.0);
        }

        assert!(world.body(handle).velocity.x > 0.0);
        assert!(world.body(handle).position.x > 0.0);
    }

    #[test]
    fn body_count_tracks_additions() {
        let mut world = PhysicsWorld::with_default_config();
        assert_eq!(world.body_count(), 0);

        world.add_body(RigidBody::dynamic(Shape::circle(1.0), Material::default()));
        assert_eq!(world.body_count(), 1);

        world.add_body(RigidBody::stationary(
            Shape::rectangle(10.0, 1.0),
            Material::default(),
        ));
        assert_eq!(world.body_count(), 2);
    }
}
