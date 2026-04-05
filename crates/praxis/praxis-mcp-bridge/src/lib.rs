//! # praxis-mcp — MCP (Model Context Protocol) Bridge
//!
//! Provides both **client** and **server** MCP capabilities for the Praxis tool engine.
//!
//! ## Server (exposing tools)
//!
//! - [`server::PraxisMcpServer`] — wraps a [`ToolRegistry`] as an MCP server
//! - [`transport`] — stdio and Streamable HTTP transport helpers
//! - [`convert`] — bidirectional type conversions (canonical ↔ MCP)
//!
//! ## Client (consuming external MCP servers)
//!
//! - [`connection`] — MCP connection management (connect to external servers)
//! - [`tool`] — [`McpTool`](tool::McpTool) wrapping external tool calls as canonical tools

pub mod connection;
pub mod convert;
pub mod server;
pub mod tool;
pub mod transport;
