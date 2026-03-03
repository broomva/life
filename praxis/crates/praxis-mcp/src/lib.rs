//! # praxis-mcp — MCP Server Bridge
//!
//! Wraps external MCP (Model Context Protocol) servers as canonical tools.
//!
//! - [`connection`] — MCP connection management
//! - [`tool`] — McpTool wrapping external tool calls

pub mod connection;
pub mod tool;
