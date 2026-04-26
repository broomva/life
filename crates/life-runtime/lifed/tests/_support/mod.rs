//! Shared test harness for lifed integration tests.
//!
//! - `mock_substrates` — tonic-test-driven fakes for arcan/lago/haima/anima.
//! - `test_env` — `TestEnv::start_with_mocks` / `start_with_real_substrates`
//!   which boots tempdir-rooted lifed + the right substrate set.

#![allow(dead_code)]

pub mod mock_substrates;
pub mod test_env;
