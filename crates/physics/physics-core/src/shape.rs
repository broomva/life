//! Shape definitions for rigid bodies.

use serde::{Deserialize, Serialize};

use crate::math::{Aabb, Vec2};

/// A collision shape in local space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    /// Circle centered at the body origin.
    Circle { radius: f64 },
    /// Convex polygon with vertices in local space (counter-clockwise winding).
    Polygon { vertices: Vec<Vec2> },
}

impl Shape {
    /// Creates a circle shape.
    pub fn circle(radius: f64) -> Self {
        Self::Circle { radius }
    }

    /// Creates a rectangle centered at the origin.
    pub fn rectangle(width: f64, height: f64) -> Self {
        let hw = width / 2.0;
        let hh = height / 2.0;
        Self::Polygon {
            vertices: vec![
                Vec2::new(-hw, -hh),
                Vec2::new(hw, -hh),
                Vec2::new(hw, hh),
                Vec2::new(-hw, hh),
            ],
        }
    }

    /// Creates a regular polygon with `n` sides inscribed in a circle of the given `radius`.
    pub fn regular_polygon(sides: usize, radius: f64) -> Self {
        assert!(sides >= 3, "polygon must have at least 3 sides");
        let step = std::f64::consts::TAU / sides as f64;
        let vertices = (0..sides)
            .map(|i| {
                let angle = step * i as f64;
                Vec2::new(angle.cos() * radius, angle.sin() * radius)
            })
            .collect();
        Self::Polygon { vertices }
    }

    /// Computes the world-space AABB for this shape given a body transform.
    pub fn aabb(&self, position: Vec2, angle: f64) -> Aabb {
        match self {
            Self::Circle { radius } => {
                let r = Vec2::new(*radius, *radius);
                Aabb::from_center_half_extents(position, r)
            }
            Self::Polygon { vertices } => {
                let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
                let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
                for v in vertices {
                    let world = v.rotate(angle) + position;
                    min.x = min.x.min(world.x);
                    min.y = min.y.min(world.y);
                    max.x = max.x.max(world.x);
                    max.y = max.y.max(world.y);
                }
                Aabb::new(min, max)
            }
        }
    }

    /// Returns the vertices transformed to world space. Empty for circles.
    pub fn world_vertices(&self, position: Vec2, angle: f64) -> Vec<Vec2> {
        match self {
            Self::Circle { .. } => vec![],
            Self::Polygon { vertices } => vertices
                .iter()
                .map(|v| v.rotate(angle) + position)
                .collect(),
        }
    }

    /// Computes the area of the shape.
    pub fn area(&self) -> f64 {
        match self {
            Self::Circle { radius } => std::f64::consts::PI * radius * radius,
            Self::Polygon { vertices } => polygon_area(vertices),
        }
    }

    /// Computes the moment of inertia about the center of mass for a given mass.
    pub fn moment_of_inertia(&self, mass: f64) -> f64 {
        match self {
            Self::Circle { radius } => 0.5 * mass * radius * radius,
            Self::Polygon { vertices } => polygon_inertia(vertices, mass),
        }
    }
}

/// Signed area of a simple polygon (positive for CCW winding).
fn polygon_area(vertices: &[Vec2]) -> f64 {
    let n = vertices.len();
    let mut area = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        area += vertices[i].cross(vertices[j]);
    }
    area.abs() / 2.0
}

/// Moment of inertia of a convex polygon about its centroid.
fn polygon_inertia(vertices: &[Vec2], mass: f64) -> f64 {
    let n = vertices.len();
    if n < 3 {
        return 0.0;
    }

    let mut numerator = 0.0;
    let mut denominator = 0.0;

    for i in 0..n {
        let j = (i + 1) % n;
        let vi = vertices[i];
        let vj = vertices[j];
        let cross = vi.cross(vj).abs();
        numerator += cross * (vi.dot(vi) + vi.dot(vj) + vj.dot(vj));
        denominator += cross;
    }

    if denominator.abs() < f64::EPSILON {
        return 0.0;
    }

    mass * numerator / (6.0 * denominator)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f64 = 1e-6;

    #[test]
    fn circle_aabb() {
        let shape = Shape::circle(2.0);
        let aabb = shape.aabb(Vec2::new(5.0, 5.0), 0.0);
        assert_eq!(aabb.min, Vec2::new(3.0, 3.0));
        assert_eq!(aabb.max, Vec2::new(7.0, 7.0));
    }

    #[test]
    fn rectangle_creation() {
        let shape = Shape::rectangle(4.0, 2.0);
        if let Shape::Polygon { vertices } = &shape {
            assert_eq!(vertices.len(), 4);
            assert_eq!(vertices[0], Vec2::new(-2.0, -1.0));
            assert_eq!(vertices[1], Vec2::new(2.0, -1.0));
            assert_eq!(vertices[2], Vec2::new(2.0, 1.0));
            assert_eq!(vertices[3], Vec2::new(-2.0, 1.0));
        } else {
            panic!("expected polygon");
        }
    }

    #[test]
    fn rectangle_aabb_axis_aligned() {
        let shape = Shape::rectangle(4.0, 2.0);
        let aabb = shape.aabb(Vec2::new(0.0, 0.0), 0.0);
        assert!((aabb.min.x - (-2.0)).abs() < EPSILON);
        assert!((aabb.min.y - (-1.0)).abs() < EPSILON);
        assert!((aabb.max.x - 2.0).abs() < EPSILON);
        assert!((aabb.max.y - 1.0).abs() < EPSILON);
    }

    #[test]
    fn circle_area() {
        let shape = Shape::circle(1.0);
        assert!((shape.area() - std::f64::consts::PI).abs() < EPSILON);
    }

    #[test]
    fn rectangle_area() {
        let shape = Shape::rectangle(4.0, 3.0);
        assert!((shape.area() - 12.0).abs() < EPSILON);
    }

    #[test]
    fn circle_inertia() {
        let shape = Shape::circle(2.0);
        let mass = 5.0;
        // I = 0.5 * m * r^2 = 0.5 * 5 * 4 = 10
        assert!((shape.moment_of_inertia(mass) - 10.0).abs() < EPSILON);
    }

    #[test]
    fn regular_polygon_sides() {
        let hex = Shape::regular_polygon(6, 1.0);
        if let Shape::Polygon { vertices } = hex {
            assert_eq!(vertices.len(), 6);
        } else {
            panic!("expected polygon");
        }
    }

    #[test]
    fn world_vertices_circle_empty() {
        let shape = Shape::circle(1.0);
        assert!(shape.world_vertices(Vec2::ZERO, 0.0).is_empty());
    }

    #[test]
    fn world_vertices_polygon_translated() {
        let shape = Shape::rectangle(2.0, 2.0);
        let verts = shape.world_vertices(Vec2::new(5.0, 5.0), 0.0);
        assert_eq!(verts.len(), 4);
        assert_eq!(verts[0], Vec2::new(4.0, 4.0));
        assert_eq!(verts[2], Vec2::new(6.0, 6.0));
    }
}
