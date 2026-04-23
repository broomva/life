//! Phase 0 scaffold — Phase 1 will fill in the test suite.
//!
//! The conformance suite provides a standard battery of tests that every
//! `HypervisorBackend` implementation MUST pass. Backends plug in via the
//! `ConformanceSuite` trait.

#![deny(unsafe_code)]

use aios_protocol::hypervisor::HypervisorBackend;

/// Marker trait for a backend under test. Phase 1 will add actual test methods.
pub trait ConformanceSuite: HypervisorBackend {}

impl<T: HypervisorBackend> ConformanceSuite for T {}

#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_compiles() {
        // Smoke test — confirms the crate builds in Phase 0.
    }
}
