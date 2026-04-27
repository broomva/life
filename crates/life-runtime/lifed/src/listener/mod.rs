//! UDS listener primitives.
//!
//! - `public` — `/run/life/life.sock`, no peer-cred extraction.
//! - `admin`  — `/run/life/life-admin.sock`, SO_PEERCRED-attached
//!   per Spec C₂ §5.3 (sub-phase C).

pub mod admin;
pub mod public;
