//! life-relayd — relay daemon library for remote agent sessions.
//!
//! Connects to broomva.tech via outbound HTTP polling, bridges local
//! agent sessions (Claude Code, Codex, Arcan) to the web UI.
//!
//! This module re-exports the daemon internals so they can be used from
//! the `life` CLI as `life relay auth|start|stop|status`.

pub mod adapters;
pub mod auth;
pub mod config;
pub mod connection;
pub mod daemon;
