//! Physical material properties for rigid bodies.

use serde::{Deserialize, Serialize};

/// Describes the physical surface properties of a body.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// Coefficient of restitution (bounciness). 0 = perfectly inelastic, 1 = perfectly elastic.
    pub restitution: f64,
    /// Static friction coefficient.
    pub static_friction: f64,
    /// Dynamic (kinetic) friction coefficient.
    pub dynamic_friction: f64,
    /// Density in kg/m^2 (used to compute mass from shape area).
    pub density: f64,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            restitution: 0.3,
            static_friction: 0.6,
            dynamic_friction: 0.4,
            density: 1.0,
        }
    }
}

impl Material {
    pub fn rubber() -> Self {
        Self {
            restitution: 0.8,
            static_friction: 0.9,
            dynamic_friction: 0.7,
            density: 1.5,
        }
    }

    pub fn ice() -> Self {
        Self {
            restitution: 0.1,
            static_friction: 0.05,
            dynamic_friction: 0.02,
            density: 0.9,
        }
    }

    pub fn steel() -> Self {
        Self {
            restitution: 0.5,
            static_friction: 0.74,
            dynamic_friction: 0.57,
            density: 7.8,
        }
    }

    pub fn bouncy() -> Self {
        Self {
            restitution: 0.95,
            static_friction: 0.3,
            dynamic_friction: 0.2,
            density: 1.0,
        }
    }
}
