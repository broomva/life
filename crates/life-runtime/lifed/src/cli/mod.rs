//! Operator CLI subcommands.
//!
//! All operator subcommands talk to the admin-plane UDS socket (default
//! `/run/life/life-admin.sock`). Sub-phase A scaffolds each subcommand to
//! print a placeholder message. Sub-phase C wires them against the real
//! admin-plane RPCs.

pub mod client;
pub mod routing_cache;
pub mod saga;
pub mod sessions;
