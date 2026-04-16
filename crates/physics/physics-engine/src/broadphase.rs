//! Broad-phase collision detection using AABB overlap tests.
//!
//! Reduces the number of expensive narrow-phase checks by culling pairs whose
//! axis-aligned bounding boxes do not overlap.

use physics_core::{BodyKind, RigidBody};

/// A candidate collision pair (indices into the body array).
#[derive(Debug, Clone, Copy)]
pub struct BroadPair {
    pub a: usize,
    pub b: usize,
}

/// Returns all pairs of bodies whose AABBs overlap.
///
/// Skips pairs where both bodies are static (they can never collide dynamically).
/// Uses an O(n^2) brute-force sweep — sufficient for small-to-medium body counts.
/// For large simulations, replace with spatial hashing or sweep-and-prune.
pub fn detect_pairs(bodies: &[RigidBody]) -> Vec<BroadPair> {
    let n = bodies.len();
    let mut pairs = Vec::new();

    // Pre-compute AABBs
    let aabbs: Vec<_> = bodies
        .iter()
        .map(|b| b.shape.aabb(b.position, b.angle))
        .collect();

    for i in 0..n {
        for j in (i + 1)..n {
            // Skip static-static pairs
            if bodies[i].kind == BodyKind::Static && bodies[j].kind == BodyKind::Static {
                continue;
            }
            if aabbs[i].overlaps(&aabbs[j]) {
                pairs.push(BroadPair { a: i, b: j });
            }
        }
    }

    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use physics_core::{Material, RigidBody, Shape, Vec2};

    #[test]
    fn overlapping_circles_detected() {
        let a = RigidBody::dynamic(Shape::circle(1.0), Material::default())
            .with_position(Vec2::new(0.0, 0.0));
        let b = RigidBody::dynamic(Shape::circle(1.0), Material::default())
            .with_position(Vec2::new(1.5, 0.0));
        let pairs = detect_pairs(&[a, b]);
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn distant_circles_not_detected() {
        let a = RigidBody::dynamic(Shape::circle(1.0), Material::default())
            .with_position(Vec2::new(0.0, 0.0));
        let b = RigidBody::dynamic(Shape::circle(1.0), Material::default())
            .with_position(Vec2::new(10.0, 0.0));
        let pairs = detect_pairs(&[a, b]);
        assert!(pairs.is_empty());
    }

    #[test]
    fn static_static_pairs_skipped() {
        let a = RigidBody::stationary(Shape::circle(5.0), Material::default())
            .with_position(Vec2::new(0.0, 0.0));
        let b = RigidBody::stationary(Shape::circle(5.0), Material::default())
            .with_position(Vec2::new(1.0, 0.0));
        let pairs = detect_pairs(&[a, b]);
        assert!(pairs.is_empty());
    }
}
