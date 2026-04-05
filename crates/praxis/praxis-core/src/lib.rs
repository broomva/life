//! # praxis-core — Sandbox Policy, Workspace Enforcement, Command Runner
//!
//! Core infrastructure for the Praxis tool execution engine.
//! Provides workspace boundary enforcement, sandbox policy,
//! the command runner abstraction, and the filesystem port.

pub mod error;
pub mod fs_port;
pub mod local_fs;
pub mod sandbox;
pub mod workspace;

pub use fs_port::{FsDirEntry, FsMetadata, FsPort};
pub use local_fs::LocalFs;
