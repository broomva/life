//! McpTool — wraps a single MCP server tool as a canonical Tool.

use aios_protocol::tool::{
    Tool, ToolCall, ToolContent, ToolContext, ToolDefinition, ToolError, ToolResult,
};
use rmcp::model::{CallToolRequestParams, RawContent};
use rmcp::service::{Peer, RoleClient};
use serde_json::json;
use std::borrow::Cow;
use std::sync::Arc;
use tokio::runtime::Handle;

/// Bridge: wraps a single MCP tool into the canonical Tool trait.
pub struct McpTool {
    definition: ToolDefinition,
    peer: Arc<Peer<RoleClient>>,
    mcp_tool_name: String,
    runtime: Handle,
}

impl McpTool {
    /// Create a new MCP tool bridge.
    pub fn new(
        definition: ToolDefinition,
        peer: Arc<Peer<RoleClient>>,
        mcp_tool_name: String,
        runtime: Handle,
    ) -> Self {
        Self {
            definition,
            peer,
            mcp_tool_name,
            runtime,
        }
    }
}

impl Tool for McpTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let arguments = call.input.as_object().cloned();

        let params = CallToolRequestParams {
            meta: None,
            name: Cow::Owned(self.mcp_tool_name.clone()),
            arguments,
            task: None,
        };

        let peer = self.peer.clone();
        let mcp_result = self
            .runtime
            .block_on(async move { peer.call_tool(params).await })
            .map_err(|e| ToolError::ExecutionFailed {
                tool_name: call.tool_name.clone(),
                message: format!("MCP call_tool failed: {e}"),
            })?;

        // Convert MCP content to canonical ToolContent
        let content: Vec<ToolContent> = mcp_result
            .content
            .iter()
            .filter_map(|c| match &c.raw {
                RawContent::Text(text) => Some(ToolContent::Text {
                    text: text.text.clone(),
                }),
                RawContent::Image(img) => Some(ToolContent::Image {
                    data: img.data.clone(),
                    mime_type: img.mime_type.clone(),
                }),
                _ => None,
            })
            .collect();

        // Build JSON output for backward compat
        let output = if let Some(structured) = &mcp_result.structured_content {
            structured.clone()
        } else {
            let texts: Vec<String> = mcp_result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect();
            json!({ "text": texts.join("\n") })
        };

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output,
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            is_error: mcp_result.is_error.unwrap_or(false),
        })
    }
}
