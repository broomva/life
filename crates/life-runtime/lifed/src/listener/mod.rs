//! UDS listener primitives.
//!
//! Sub-phase A ships only the public-plane listener. The admin-plane listener
//! lands in C1 (Spec C₂ §5.3 — SO_PEERCRED + group membership + pidfd).

pub mod public;
// pub mod admin;     // sub-phase C (C1)
