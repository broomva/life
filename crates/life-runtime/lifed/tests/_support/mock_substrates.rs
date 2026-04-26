//! Mock arcan/lago/haima/anima daemons backed by tonic test infrastructure.
//!
//! Sub-phase A uses these to validate lifed's handler shape and
//! routing/auth/observability scaffolding without depending on the real
//! substrate dev cluster. Sub-phase B replaces these mocks with real-substrate
//! integration via the four `*-proxy` crates.

#![allow(dead_code)]

// Re-export the in-crate dev mocks. The same types are used by lifed's
// bootstrap to drive its mock-mode daemon, and by integration tests to assert
// that the right substrate calls were made.
pub use lifed::dev_mocks::MockSubstrates;
#[allow(unused_imports)]
pub use lifed::dev_mocks::{MockAnima, MockArcan, MockHaima, MockLago};
