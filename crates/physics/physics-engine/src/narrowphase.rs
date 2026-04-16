//! Narrow-phase collision detection.
//!
//! Implements exact collision tests between shape pairs:
//! - Circle vs Circle
//! - Circle vs Convex Polygon
//! - Convex Polygon vs Convex Polygon (Separating Axis Theorem)
//!
//! Contact manifolds contain 1–2 contact points with world-space positions
//! and penetration depths.

use physics_core::{ContactManifold, ContactPoint, Shape, Vec2};

/// Performs narrow-phase collision detection between two shapes.
///
/// Returns a contact manifold if the shapes overlap, with the normal
/// pointing from body A toward body B.
pub fn detect(
    idx_a: usize,
    shape_a: &Shape,
    pos_a: Vec2,
    angle_a: f64,
    idx_b: usize,
    shape_b: &Shape,
    pos_b: Vec2,
    angle_b: f64,
) -> Option<ContactManifold> {
    let result = match (shape_a, shape_b) {
        (Shape::Circle { radius: ra }, Shape::Circle { radius: rb }) => {
            circle_vs_circle(pos_a, *ra, pos_b, *rb)
        }
        (Shape::Circle { radius }, Shape::Polygon { vertices }) => {
            let world_verts = world_vertices(vertices, pos_b, angle_b);
            circle_vs_polygon(pos_a, *radius, &world_verts)
        }
        (Shape::Polygon { vertices }, Shape::Circle { radius }) => {
            let world_verts = world_vertices(vertices, pos_a, angle_a);
            // Detect with circle as "A" then flip the result
            circle_vs_polygon(pos_b, *radius, &world_verts).map(|mut r| {
                r.normal = -r.normal;
                r
            })
        }
        (Shape::Polygon { vertices: va }, Shape::Polygon { vertices: vb }) => {
            let wa = world_vertices(va, pos_a, angle_a);
            let wb = world_vertices(vb, pos_b, angle_b);
            polygon_vs_polygon(&wa, &wb)
        }
    };

    result.map(|r| ContactManifold {
        body_a: idx_a,
        body_b: idx_b,
        normal: r.normal,
        contacts: r.contacts,
    })
}

// ── Internal result type ──────────────────────────────────────────────────────

struct CollisionResult {
    normal: Vec2,
    contacts: Vec<ContactPoint>,
}

// ── Circle vs Circle ──────────────────────────────────────────────────────────

fn circle_vs_circle(
    pos_a: Vec2,
    radius_a: f64,
    pos_b: Vec2,
    radius_b: f64,
) -> Option<CollisionResult> {
    let d = pos_b - pos_a;
    let dist_sq = d.magnitude_squared();
    let sum_r = radius_a + radius_b;

    if dist_sq > sum_r * sum_r {
        return None;
    }

    let dist = dist_sq.sqrt();
    let normal = if dist > f64::EPSILON {
        d / dist
    } else {
        Vec2::RIGHT // arbitrary if coincident
    };
    let penetration = sum_r - dist;
    let contact_pos = pos_a + normal * (radius_a - penetration / 2.0);

    Some(CollisionResult {
        normal,
        contacts: vec![ContactPoint {
            position: contact_pos,
            penetration,
        }],
    })
}

// ── Circle vs Polygon (SAT-based) ────────────────────────────────────────────

fn circle_vs_polygon(
    circle_pos: Vec2,
    radius: f64,
    poly_verts: &[Vec2],
) -> Option<CollisionResult> {
    let n = poly_verts.len();
    if n < 3 {
        return None;
    }

    let mut min_pen = f64::INFINITY;
    let mut best_axis = Vec2::ZERO;

    // Test polygon edge normals
    for i in 0..n {
        let j = (i + 1) % n;
        let edge = poly_verts[j] - poly_verts[i];
        let axis = Vec2::new(edge.y, -edge.x).normalized();
        if axis.magnitude_squared() < f64::EPSILON {
            continue;
        }

        let (min_p, max_p) = project_vertices(poly_verts, axis);
        let center_proj = circle_pos.dot(axis);
        let min_c = center_proj - radius;
        let max_c = center_proj + radius;

        if min_p >= max_c || min_c >= max_p {
            return None;
        }

        let overlap = max_c.min(max_p) - min_c.max(min_p);
        if overlap < min_pen {
            min_pen = overlap;
            best_axis = axis;
        }
    }

    // Test axis from closest vertex to circle center
    let closest = poly_verts
        .iter()
        .min_by(|a, b| {
            a.distance_squared(circle_pos)
                .partial_cmp(&b.distance_squared(circle_pos))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap();

    let vertex_axis = (*closest - circle_pos).normalized();
    if vertex_axis.magnitude_squared() > f64::EPSILON {
        let (min_p, max_p) = project_vertices(poly_verts, vertex_axis);
        let center_proj = circle_pos.dot(vertex_axis);
        let min_c = center_proj - radius;
        let max_c = center_proj + radius;

        if min_p >= max_c || min_c >= max_p {
            return None;
        }

        let overlap = max_c.min(max_p) - min_c.max(min_p);
        if overlap < min_pen {
            min_pen = overlap;
            best_axis = vertex_axis;
        }
    }

    // Ensure normal points from circle to polygon
    let poly_center = centroid(poly_verts);
    if best_axis.dot(poly_center - circle_pos) < 0.0 {
        best_axis = -best_axis;
    }

    let contact_pos = circle_pos + best_axis * (radius - min_pen / 2.0);

    Some(CollisionResult {
        normal: best_axis,
        contacts: vec![ContactPoint {
            position: contact_pos,
            penetration: min_pen,
        }],
    })
}

// ── Polygon vs Polygon (SAT + Sutherland-Hodgman clipping) ────────────────────

fn polygon_vs_polygon(verts_a: &[Vec2], verts_b: &[Vec2]) -> Option<CollisionResult> {
    if verts_a.len() < 3 || verts_b.len() < 3 {
        return None;
    }

    let mut min_pen = f64::INFINITY;
    let mut best_axis = Vec2::ZERO;

    // Test A's edge normals
    if !test_polygon_axes(verts_a, verts_b, &mut min_pen, &mut best_axis) {
        return None;
    }

    // Test B's edge normals
    if !test_polygon_axes(verts_b, verts_a, &mut min_pen, &mut best_axis) {
        return None;
    }

    // Ensure normal points from A to B
    let center_a = centroid(verts_a);
    let center_b = centroid(verts_b);
    if best_axis.dot(center_b - center_a) < 0.0 {
        best_axis = -best_axis;
    }

    // Generate contact points via edge clipping
    let contacts = clip_contacts(verts_a, verts_b, best_axis, min_pen);
    if contacts.is_empty() {
        return None;
    }

    Some(CollisionResult {
        normal: best_axis,
        contacts,
    })
}

/// Tests all edge normals of `source` against the projection of both polygons.
/// Returns `false` if a separating axis is found.
fn test_polygon_axes(
    source: &[Vec2],
    other: &[Vec2],
    min_pen: &mut f64,
    best_axis: &mut Vec2,
) -> bool {
    let n = source.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let edge = source[j] - source[i];
        let axis = Vec2::new(edge.y, -edge.x).normalized();
        if axis.magnitude_squared() < f64::EPSILON {
            continue;
        }

        let (min_a, max_a) = project_vertices(source, axis);
        let (min_b, max_b) = project_vertices(other, axis);

        if min_a >= max_b || min_b >= max_a {
            return false;
        }

        let overlap = max_a.min(max_b) - min_a.max(min_b);
        if overlap < *min_pen {
            *min_pen = overlap;
            *best_axis = axis;
        }
    }
    true
}

/// Sutherland-Hodgman edge clipping to produce contact points.
fn clip_contacts(verts_a: &[Vec2], verts_b: &[Vec2], normal: Vec2, _pen: f64) -> Vec<ContactPoint> {
    // Find the reference edge on A (most aligned with normal)
    let ref_idx = find_best_edge(verts_a, normal);
    let ref_v1 = verts_a[ref_idx];
    let ref_v2 = verts_a[(ref_idx + 1) % verts_a.len()];

    // Find the incident edge on B (most anti-aligned with normal)
    let inc_idx = find_best_edge(verts_b, -normal);
    let inc_v1 = verts_b[inc_idx];
    let inc_v2 = verts_b[(inc_idx + 1) % verts_b.len()];

    let ref_dir = (ref_v2 - ref_v1).normalized();

    // Clip incident edge against reference face side planes
    let mut clipped = vec![inc_v1, inc_v2];
    clipped = clip_segment(clipped, -ref_dir, -ref_dir.dot(ref_v1));
    if clipped.is_empty() {
        return vec![];
    }
    clipped = clip_segment(clipped, ref_dir, ref_dir.dot(ref_v2));
    if clipped.is_empty() {
        return vec![];
    }

    // Keep only points behind the reference face
    let ref_offset = normal.dot(ref_v1);
    clipped
        .into_iter()
        .filter_map(|p| {
            let sep = normal.dot(p) - ref_offset;
            if sep <= 0.0 {
                Some(ContactPoint {
                    position: p,
                    penetration: -sep,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Finds the edge whose outward normal is most aligned with `direction`.
fn find_best_edge(verts: &[Vec2], direction: Vec2) -> usize {
    let n = verts.len();
    let mut best = 0;
    let mut best_dot = f64::NEG_INFINITY;
    for i in 0..n {
        let j = (i + 1) % n;
        let edge = verts[j] - verts[i];
        let edge_normal = Vec2::new(edge.y, -edge.x).normalized();
        let d = edge_normal.dot(direction);
        if d > best_dot {
            best_dot = d;
            best = i;
        }
    }
    best
}

/// Clips a segment (2 points) against a half-plane defined by `normal . x >= offset`.
fn clip_segment(points: Vec<Vec2>, plane_normal: Vec2, plane_offset: f64) -> Vec<Vec2> {
    if points.len() < 2 {
        return points;
    }

    let d0 = plane_normal.dot(points[0]) - plane_offset;
    let d1 = plane_normal.dot(points[1]) - plane_offset;

    let mut out = Vec::with_capacity(2);

    if d0 >= 0.0 {
        out.push(points[0]);
    }
    if d1 >= 0.0 {
        out.push(points[1]);
    }

    // If points are on opposite sides, add the intersection
    if d0 * d1 < 0.0 {
        let t = d0 / (d0 - d1);
        out.push(points[0].lerp(points[1], t));
    }

    out
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn world_vertices(local: &[Vec2], pos: Vec2, angle: f64) -> Vec<Vec2> {
    local.iter().map(|v| v.rotate(angle) + pos).collect()
}

fn project_vertices(vertices: &[Vec2], axis: Vec2) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for v in vertices {
        let proj = v.dot(axis);
        min = min.min(proj);
        max = max.max(proj);
    }
    (min, max)
}

fn centroid(vertices: &[Vec2]) -> Vec2 {
    let n = vertices.len() as f64;
    let sum = vertices.iter().fold(Vec2::ZERO, |acc, v| acc + *v);
    sum / n
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use physics_core::Shape;

    #[test]
    fn circle_circle_collision() {
        let result = circle_vs_circle(Vec2::new(0.0, 0.0), 1.0, Vec2::new(1.5, 0.0), 1.0);
        let r = result.expect("should collide");
        assert!((r.normal.x - 1.0).abs() < 1e-10);
        assert!(r.normal.y.abs() < 1e-10);
        assert!((r.contacts[0].penetration - 0.5).abs() < 1e-10);
    }

    #[test]
    fn circle_circle_no_collision() {
        let result = circle_vs_circle(Vec2::new(0.0, 0.0), 1.0, Vec2::new(3.0, 0.0), 1.0);
        assert!(result.is_none());
    }

    #[test]
    fn circle_circle_touching() {
        let result = circle_vs_circle(Vec2::new(0.0, 0.0), 1.0, Vec2::new(2.0, 0.0), 1.0);
        // Exactly touching means penetration = 0, which our test treats as no collision
        // (dist_sq > sum_r * sum_r is false when equal, so it IS detected)
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.contacts[0].penetration.abs() < 1e-10);
    }

    #[test]
    fn polygon_polygon_collision() {
        // Two overlapping unit squares
        let sq_a = Shape::rectangle(2.0, 2.0);
        let sq_b = Shape::rectangle(2.0, 2.0);
        let wa = sq_a.world_vertices(Vec2::new(0.0, 0.0), 0.0);
        let wb = sq_b.world_vertices(Vec2::new(1.5, 0.0), 0.0);

        let result = polygon_vs_polygon(&wa, &wb);
        let r = result.expect("squares should overlap");
        assert!(r.contacts[0].penetration > 0.0);
        // Normal should roughly point in the +x direction (from A to B)
        assert!(r.normal.x > 0.5);
    }

    #[test]
    fn polygon_polygon_no_collision() {
        let sq_a = Shape::rectangle(2.0, 2.0);
        let sq_b = Shape::rectangle(2.0, 2.0);
        let wa = sq_a.world_vertices(Vec2::new(0.0, 0.0), 0.0);
        let wb = sq_b.world_vertices(Vec2::new(5.0, 0.0), 0.0);

        let result = polygon_vs_polygon(&wa, &wb);
        assert!(result.is_none());
    }

    #[test]
    fn circle_polygon_collision() {
        let poly = Shape::rectangle(2.0, 2.0);
        let world_verts = poly.world_vertices(Vec2::new(0.0, 0.0), 0.0);

        let result = circle_vs_polygon(Vec2::new(1.5, 0.0), 1.0, &world_verts);
        let r = result.expect("circle should hit rectangle");
        assert!(r.contacts[0].penetration > 0.0);
    }

    #[test]
    fn circle_polygon_no_collision() {
        let poly = Shape::rectangle(2.0, 2.0);
        let world_verts = poly.world_vertices(Vec2::new(0.0, 0.0), 0.0);

        let result = circle_vs_polygon(Vec2::new(5.0, 0.0), 1.0, &world_verts);
        assert!(result.is_none());
    }

    #[test]
    fn detect_full_pipeline() {
        let shape_a = Shape::circle(1.0);
        let shape_b = Shape::rectangle(2.0, 2.0);

        let manifold = detect(
            0,
            &shape_a,
            Vec2::new(0.0, 0.0),
            0.0,
            1,
            &shape_b,
            Vec2::new(1.5, 0.0),
            0.0,
        );

        let m = manifold.expect("should detect collision");
        assert_eq!(m.body_a, 0);
        assert_eq!(m.body_b, 1);
        assert!(!m.contacts.is_empty());
    }
}
