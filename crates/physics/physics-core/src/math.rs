//! 2D math primitives: vectors and axis-aligned bounding boxes.

use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

// ── Vec2 ──────────────────────────────────────────────────────────────────────

/// A 2D vector with `f64` components.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Default for Vec2 {
    fn default() -> Self {
        Self::ZERO
    }
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
    pub const ONE: Self = Self { x: 1.0, y: 1.0 };
    pub const UP: Self = Self { x: 0.0, y: 1.0 };
    pub const DOWN: Self = Self { x: 0.0, y: -1.0 };
    pub const LEFT: Self = Self { x: -1.0, y: 0.0 };
    pub const RIGHT: Self = Self { x: 1.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Dot product of two vectors.
    #[inline]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// 2D cross product — returns the z-component of the 3D cross product.
    /// Positive when `other` is counter-clockwise from `self`.
    #[inline]
    pub fn cross(self, other: Self) -> f64 {
        self.x * other.y - self.y * other.x
    }

    /// Cross product of a scalar and a vector: `s x v = (-s*v.y, s*v.x)`.
    /// Useful for computing tangential velocity from angular velocity.
    #[inline]
    pub fn cross_scalar(s: f64, v: Self) -> Self {
        Self::new(-s * v.y, s * v.x)
    }

    #[inline]
    pub fn magnitude_squared(self) -> f64 {
        self.dot(self)
    }

    #[inline]
    pub fn magnitude(self) -> f64 {
        self.magnitude_squared().sqrt()
    }

    /// Returns the unit vector, or `Vec2::ZERO` if the magnitude is near zero.
    pub fn normalized(self) -> Self {
        let mag = self.magnitude();
        if mag < f64::EPSILON {
            Self::ZERO
        } else {
            self / mag
        }
    }

    #[inline]
    pub fn distance(self, other: Self) -> f64 {
        (self - other).magnitude()
    }

    #[inline]
    pub fn distance_squared(self, other: Self) -> f64 {
        (self - other).magnitude_squared()
    }

    /// Returns the vector rotated 90 degrees counter-clockwise.
    #[inline]
    pub fn perpendicular(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// Rotates the vector by `angle` radians counter-clockwise.
    pub fn rotate(self, angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(self.x * cos - self.y * sin, self.x * sin + self.y * cos)
    }

    /// Linear interpolation between `self` and `other` at parameter `t`.
    #[inline]
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self * (1.0 - t) + other * t
    }
}

// ── Operator implementations ──────────────────────────────────────────────────

impl Add for Vec2 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f64) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Mul<Vec2> for f64 {
    type Output = Vec2;
    #[inline]
    fn mul(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self * rhs.x, self * rhs.y)
    }
}

impl Div<f64> for Vec2 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f64) -> Self {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl Neg for Vec2 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl MulAssign<f64> for Vec2 {
    #[inline]
    fn mul_assign(&mut self, rhs: f64) {
        self.x *= rhs;
        self.y *= rhs;
    }
}

impl DivAssign<f64> for Vec2 {
    #[inline]
    fn div_assign(&mut self, rhs: f64) {
        self.x /= rhs;
        self.y /= rhs;
    }
}

// ── Aabb ──────────────────────────────────────────────────────────────────────

/// Axis-aligned bounding box defined by min/max corners.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    pub fn from_center_half_extents(center: Vec2, half: Vec2) -> Self {
        Self {
            min: center - half,
            max: center + half,
        }
    }

    /// Returns `true` if this AABB overlaps `other` (inclusive on boundaries).
    pub fn overlaps(&self, other: &Self) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
    }

    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    pub fn half_extents(&self) -> Vec2 {
        (self.max - self.min) * 0.5
    }

    /// Returns the smallest AABB that contains both `self` and `other`.
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            min: Vec2::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Vec2::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    const EPSILON: f64 = 1e-10;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < EPSILON
    }

    fn vec2_approx_eq(a: Vec2, b: Vec2) -> bool {
        approx_eq(a.x, b.x) && approx_eq(a.y, b.y)
    }

    #[test]
    fn vec2_add() {
        let a = Vec2::new(1.0, 2.0);
        let b = Vec2::new(3.0, 4.0);
        assert_eq!(a + b, Vec2::new(4.0, 6.0));
    }

    #[test]
    fn vec2_sub() {
        let a = Vec2::new(5.0, 7.0);
        let b = Vec2::new(2.0, 3.0);
        assert_eq!(a - b, Vec2::new(3.0, 4.0));
    }

    #[test]
    fn vec2_mul_scalar() {
        let v = Vec2::new(2.0, 3.0);
        assert_eq!(v * 2.0, Vec2::new(4.0, 6.0));
        assert_eq!(2.0 * v, Vec2::new(4.0, 6.0));
    }

    #[test]
    fn vec2_dot_product() {
        let a = Vec2::new(1.0, 0.0);
        let b = Vec2::new(0.0, 1.0);
        assert!(approx_eq(a.dot(b), 0.0));

        let c = Vec2::new(3.0, 4.0);
        let d = Vec2::new(2.0, 1.0);
        assert!(approx_eq(c.dot(d), 10.0));
    }

    #[test]
    fn vec2_cross_product() {
        let a = Vec2::new(1.0, 0.0);
        let b = Vec2::new(0.0, 1.0);
        assert!(approx_eq(a.cross(b), 1.0));
        assert!(approx_eq(b.cross(a), -1.0));
    }

    #[test]
    fn vec2_magnitude() {
        let v = Vec2::new(3.0, 4.0);
        assert!(approx_eq(v.magnitude(), 5.0));
        assert!(approx_eq(v.magnitude_squared(), 25.0));
    }

    #[test]
    fn vec2_normalized() {
        let v = Vec2::new(3.0, 4.0);
        let n = v.normalized();
        assert!(approx_eq(n.magnitude(), 1.0));
        assert!(approx_eq(n.x, 0.6));
        assert!(approx_eq(n.y, 0.8));

        // Zero vector normalization
        assert_eq!(Vec2::ZERO.normalized(), Vec2::ZERO);
    }

    #[test]
    fn vec2_rotate() {
        let v = Vec2::new(1.0, 0.0);
        let rotated = v.rotate(FRAC_PI_2);
        assert!(vec2_approx_eq(rotated, Vec2::new(0.0, 1.0)));
    }

    #[test]
    fn vec2_perpendicular() {
        let v = Vec2::new(1.0, 0.0);
        assert_eq!(v.perpendicular(), Vec2::new(0.0, 1.0));
    }

    #[test]
    fn vec2_distance() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(3.0, 4.0);
        assert!(approx_eq(a.distance(b), 5.0));
    }

    #[test]
    fn vec2_lerp() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 10.0);
        assert_eq!(a.lerp(b, 0.5), Vec2::new(5.0, 5.0));
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
    }

    #[test]
    fn vec2_cross_scalar() {
        let v = Vec2::new(1.0, 0.0);
        let result = Vec2::cross_scalar(1.0, v);
        assert_eq!(result, Vec2::new(0.0, 1.0));
    }

    #[test]
    fn aabb_overlaps() {
        let a = Aabb::new(Vec2::new(0.0, 0.0), Vec2::new(2.0, 2.0));
        let b = Aabb::new(Vec2::new(1.0, 1.0), Vec2::new(3.0, 3.0));
        assert!(a.overlaps(&b));
        assert!(b.overlaps(&a));
    }

    #[test]
    fn aabb_no_overlap() {
        let a = Aabb::new(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0));
        let b = Aabb::new(Vec2::new(2.0, 2.0), Vec2::new(3.0, 3.0));
        assert!(!a.overlaps(&b));
    }

    #[test]
    fn aabb_touching_edges_overlap() {
        let a = Aabb::new(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0));
        let b = Aabb::new(Vec2::new(1.0, 0.0), Vec2::new(2.0, 1.0));
        assert!(a.overlaps(&b));
    }

    #[test]
    fn aabb_merge() {
        let a = Aabb::new(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0));
        let b = Aabb::new(Vec2::new(2.0, 2.0), Vec2::new(3.0, 3.0));
        let merged = a.merge(&b);
        assert_eq!(merged.min, Vec2::new(0.0, 0.0));
        assert_eq!(merged.max, Vec2::new(3.0, 3.0));
    }

    #[test]
    fn aabb_center_and_half_extents() {
        let aabb = Aabb::new(Vec2::new(1.0, 2.0), Vec2::new(5.0, 6.0));
        assert_eq!(aabb.center(), Vec2::new(3.0, 4.0));
        assert_eq!(aabb.half_extents(), Vec2::new(2.0, 2.0));
    }
}
