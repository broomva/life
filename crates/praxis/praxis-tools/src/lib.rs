//! # praxis-tools — Canonical Tool Implementations
//!
//! Implements the standard tools available to Agent OS runtimes:
//!
//! - [`fs`] — ReadFile, WriteFile, ListDir, Glob, Grep
//! - [`edit`] — EditFile with hashline (Blake3) content-addressed editing
//! - [`shell`] — Bash command execution
//! - [`memory`] — Agent memory read/write (file-based)
//! - [`remote`] — Remote command execution via [`arcan_sandbox::SandboxProvider`]
//! - [`belief`] — Four-dimensional [`belief::BeliefWriteToken`] write path,
//!   belief store, and revision-graph traversal

pub mod belief;
pub mod edit;
pub mod fs;
pub mod memory;
pub mod remote;
pub mod shell;

pub use belief::{
    BeliefRecord, BeliefStore, BeliefWriteError, BeliefWriteRoute, CapabilityGrant,
    RevisionChainEntry, SupersessionView, revision_masks_contradiction, route_write,
};
pub use remote::RemoteCommandRunner;
