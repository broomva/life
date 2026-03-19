//! MCP Server — exposes canonical Praxis tools over the Model Context Protocol.
//!
//! [`PraxisMcpServer`] bridges the canonical [`ToolRegistry`] to the MCP protocol
//! via rmcp's [`ServerHandler`] trait. Any tools registered in the registry are
//! automatically exposed as MCP tools to connected clients.
//!
//! # Usage
//!
//! ```no_run
//! use aios_protocol::tool::ToolRegistry;
//! use praxis_mcp::server::PraxisMcpServer;
//! use rmcp::ServiceExt;
//! use rmcp::transport::io::stdio;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let registry = ToolRegistry::default();
//! let server = PraxisMcpServer::new(registry);
//! server.serve(stdio()).await?.waiting().await?;
//! # Ok(())
//! # }
//! ```

use crate::convert::{definition_to_mcp_tool, tool_result_to_call_result};
use aios_protocol::tool::{ToolCall, ToolContext, ToolRegistry};
use rmcp::ErrorData as McpError;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool as McpToolDef,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::Value;
use std::future::Future;
use tracing::{info, warn};

/// MCP server that exposes all tools from a canonical [`ToolRegistry`].
///
/// Implements rmcp's [`ServerHandler`] trait to serve tools over MCP protocol.
/// The server is transport-agnostic — use it with stdio, Streamable HTTP, or
/// any transport that rmcp supports.
pub struct PraxisMcpServer {
    registry: ToolRegistry,
    server_name: String,
    server_version: String,
}

impl PraxisMcpServer {
    /// Create a new MCP server with the given tool registry.
    pub fn new(registry: ToolRegistry) -> Self {
        Self {
            registry,
            server_name: "praxis".to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Set a custom server name (default: "praxis").
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = name.into();
        self
    }

    /// Set a custom server version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.server_version = version.into();
        self
    }

    /// Access the underlying tool registry.
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Build the MCP tool list from the registry.
    pub fn mcp_tools(&self) -> Vec<McpToolDef> {
        self.registry
            .definitions()
            .iter()
            .map(definition_to_mcp_tool)
            .collect()
    }
}

impl ServerHandler for PraxisMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: Default::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: self.server_name.clone(),
                version: self.server_version.clone(),
                title: Some("Praxis Tool Engine".to_string()),
                description: Some(
                    "Canonical tool execution engine for the Life Agent OS".to_string(),
                ),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "Praxis exposes filesystem, shell, editing, and memory tools. \
                 All filesystem operations are sandboxed within the workspace root."
                    .to_string(),
            ),
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.mcp_tools();
        info!(tool_count = tools.len(), "MCP tools/list");
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let tool_name = request.name.to_string();
        let arguments = request.arguments.unwrap_or_default();

        let span = tracing::info_span!(
            "mcp_server_call_tool",
            tool = %tool_name,
            mcp.duration_ms = tracing::field::Empty,
            mcp.is_error = tracing::field::Empty,
        );
        let _guard = span.enter();
        let start = std::time::Instant::now();

        let result = match self.registry.get(&tool_name) {
            Some(tool) => {
                let call = ToolCall {
                    call_id: uuid::Uuid::new_v4().to_string(),
                    tool_name: tool_name.clone(),
                    input: Value::Object(arguments),
                    requested_capabilities: vec![],
                };

                let ctx = ToolContext {
                    run_id: "mcp".to_string(),
                    session_id: "mcp-session".to_string(),
                    iteration: 0,
                };

                match tool.execute(&call, &ctx) {
                    Ok(result) => {
                        let duration_ms = start.elapsed().as_millis() as u64;
                        span.record("mcp.duration_ms", duration_ms);
                        span.record("mcp.is_error", result.is_error);
                        info!(
                            tool = %tool_name,
                            duration_ms,
                            is_error = result.is_error,
                            "MCP tool call completed"
                        );
                        Ok(tool_result_to_call_result(&result))
                    }
                    Err(e) => {
                        let duration_ms = start.elapsed().as_millis() as u64;
                        span.record("mcp.duration_ms", duration_ms);
                        span.record("mcp.is_error", true);
                        warn!(
                            tool = %tool_name,
                            error = %e,
                            duration_ms,
                            "MCP tool call failed"
                        );
                        Ok(CallToolResult::error(vec![rmcp::model::Content::text(
                            e.to_string(),
                        )]))
                    }
                }
            }
            None => {
                warn!(tool = %tool_name, "MCP tool not found");
                Err(McpError::invalid_params(
                    format!("tool not found: {tool_name}"),
                    None,
                ))
            }
        };

        std::future::ready(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::tool::{
        Tool, ToolAnnotations, ToolCall, ToolContext, ToolDefinition, ToolError, ToolResult,
    };
    use serde_json::json;

    /// A simple echo tool for testing.
    struct EchoTool;

    impl Tool for EchoTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "echo".into(),
                description: "Echoes the input message".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string", "description": "Message to echo" }
                    },
                    "required": ["message"]
                }),
                title: Some("Echo".into()),
                output_schema: None,
                annotations: Some(ToolAnnotations {
                    read_only: true,
                    idempotent: true,
                    ..Default::default()
                }),
                category: Some("test".into()),
                tags: vec!["test".into()],
                timeout_secs: Some(10),
            }
        }

        fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
            let message = call
                .input
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("(empty)");
            Ok(ToolResult::text(&call.call_id, &call.tool_name, message))
        }
    }

    /// A tool that always fails.
    struct FailTool;

    impl Tool for FailTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "fail".into(),
                description: "Always fails".into(),
                input_schema: json!({"type": "object"}),
                title: None,
                output_schema: None,
                annotations: None,
                category: None,
                tags: vec![],
                timeout_secs: None,
            }
        }

        fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
            Err(ToolError::ExecutionFailed {
                tool_name: call.tool_name.clone(),
                message: "intentional failure".into(),
            })
        }
    }

    fn test_server() -> PraxisMcpServer {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);
        registry.register(FailTool);
        PraxisMcpServer::new(registry)
    }

    #[test]
    fn server_info() {
        let server = test_server();
        let info = server.get_info();
        assert_eq!(info.server_info.name, "praxis");
        assert!(info.capabilities.tools.is_some());
        assert!(info.instructions.is_some());
    }

    #[test]
    fn server_with_custom_name() {
        let server = test_server().with_name("my-agent").with_version("2.0.0");
        let info = server.get_info();
        assert_eq!(info.server_info.name, "my-agent");
        assert_eq!(info.server_info.version, "2.0.0");
    }

    #[test]
    fn mcp_tools_lists_all_registered() {
        let server = test_server();
        let tools = server.mcp_tools();
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"fail"));
    }

    #[test]
    fn mcp_tools_preserves_annotations() {
        let server = test_server();
        let tools = server.mcp_tools();
        let echo = tools.iter().find(|t| t.name == "echo").unwrap();
        let ann = echo.annotations.as_ref().unwrap();
        assert_eq!(ann.read_only_hint, Some(true));
        assert_eq!(ann.idempotent_hint, Some(true));
        assert_eq!(ann.destructive_hint, Some(false));
    }

    #[test]
    fn empty_registry_lists_no_tools() {
        let server = PraxisMcpServer::new(ToolRegistry::default());
        let tools = server.mcp_tools();
        assert!(tools.is_empty());
    }

    // --- Tool dispatch tests: exercise the same code path as MCP call_tool ---

    #[test]
    fn call_tool_echo_dispatches_correctly() {
        let server = test_server();
        let tool = server.registry().get("echo").unwrap();
        let call = ToolCall::new("echo", json!({"message": "hello world"}), vec![]);
        let ctx = ToolContext {
            run_id: "test".into(),
            session_id: "test".into(),
            iteration: 0,
        };
        let result = tool.execute(&call, &ctx).unwrap();
        assert!(!result.is_error);

        let mcp_result = crate::convert::tool_result_to_call_result(&result);
        assert_eq!(mcp_result.is_error, Some(false));
        let text = mcp_result.content.first().unwrap().as_text().unwrap();
        assert_eq!(text.text, "hello world");
    }

    #[test]
    fn call_tool_fail_returns_error() {
        let server = test_server();
        let tool = server.registry().get("fail").unwrap();
        let call = ToolCall::new("fail", json!({}), vec![]);
        let ctx = ToolContext {
            run_id: "test".into(),
            session_id: "test".into(),
            iteration: 0,
        };
        let err = tool.execute(&call, &ctx).unwrap_err();
        assert!(err.to_string().contains("intentional failure"));
    }

    #[test]
    fn registry_lookup_missing_returns_none() {
        let server = test_server();
        assert!(server.registry().get("nonexistent").is_none());
    }

    // --- Full MCP protocol tests via in-process duplex transport ---

    #[tokio::test]
    async fn mcp_protocol_echo_roundtrip() {
        use rmcp::ServiceExt;

        let server = test_server();
        let (s1, s2) = tokio::io::duplex(8192);

        tokio::spawn(async move {
            let running = server.serve(s1).await.unwrap();
            let _ = running.waiting().await;
        });

        let client = ().serve(s2).await.unwrap();

        let result = client
            .peer()
            .call_tool(CallToolRequestParams {
                name: std::borrow::Cow::Borrowed("echo"),
                arguments: Some(
                    json!({"message": "protocol test"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
                meta: None,
                task: None,
            })
            .await
            .unwrap();

        assert_eq!(result.is_error, Some(false));
        let text = result.content.first().unwrap().as_text().unwrap();
        assert_eq!(text.text, "protocol test");
    }

    #[tokio::test]
    async fn mcp_protocol_error_propagation() {
        use rmcp::ServiceExt;

        let server = test_server();
        let (s1, s2) = tokio::io::duplex(8192);

        tokio::spawn(async move {
            let running = server.serve(s1).await.unwrap();
            let _ = running.waiting().await;
        });

        let client = ().serve(s2).await.unwrap();

        // Tool that exists but always fails → returns error content, not protocol error
        let result = client
            .peer()
            .call_tool(CallToolRequestParams {
                name: std::borrow::Cow::Borrowed("fail"),
                arguments: None,
                meta: None,
                task: None,
            })
            .await
            .unwrap();

        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn mcp_protocol_not_found() {
        use rmcp::ServiceExt;

        let server = test_server();
        let (s1, s2) = tokio::io::duplex(8192);

        tokio::spawn(async move {
            let running = server.serve(s1).await.unwrap();
            let _ = running.waiting().await;
        });

        let client = ().serve(s2).await.unwrap();

        // Nonexistent tool → protocol-level error
        let err = client
            .peer()
            .call_tool(CallToolRequestParams {
                name: std::borrow::Cow::Borrowed("nonexistent"),
                arguments: None,
                meta: None,
                task: None,
            })
            .await
            .unwrap_err();

        assert!(err.to_string().contains("tool not found"), "got: {err}");
    }

    #[tokio::test]
    async fn mcp_protocol_list_tools() {
        use rmcp::ServiceExt;

        let server = test_server();
        let (s1, s2) = tokio::io::duplex(8192);

        tokio::spawn(async move {
            let running = server.serve(s1).await.unwrap();
            let _ = running.waiting().await;
        });

        let client = ().serve(s2).await.unwrap();
        let result = client.peer().list_tools(None).await.unwrap();

        assert_eq!(result.tools.len(), 2);
        let names: Vec<&str> = result.tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"fail"));
    }
}
