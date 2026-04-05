//! # praxis-skills — SKILL.md Discovery and Activation
//!
//! Discovers skills from SKILL.md files and provides a registry
//! for activating skills during agent execution.
//!
//! - [`registry`] — SkillRegistry for discovery and lookup
//! - [`parser`] — SKILL.md frontmatter parser

pub mod parser;
pub mod registry;
