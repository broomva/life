//! Bidirectional conversions between canonical Agent OS types and rmcp MCP types.
//!
//! - [`definition_to_mcp_tool`]: canonical [`ToolDefinition`] → rmcp [`McpToolDef`]
//! - [`mcp_result_to_call_result`]: canonical [`ToolResult`] → rmcp [`CallToolResult`]

use aios_protocol::tool::{
    ToolAnnotations as CanonicalAnnotations, ToolContent, ToolDefinition, ToolResult,
};
use rmcp::model::{CallToolResult, Content, Tool as McpToolDef, ToolAnnotations as McpAnnotations};
use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;

/// Convert a canonical [`ToolDefinition`] to an rmcp [`McpToolDef`].
///
/// The MCP protocol requires `input_schema` to be a JSON Object (`Map<String, Value>`).
/// If the canonical schema is not an object, this wraps it under `{"type": "object"}`.
pub fn definition_to_mcp_tool(def: &ToolDefinition) -> McpToolDef {
    let input_schema = match &def.input_schema {
        Value::Object(map) => Arc::new(map.clone()),
        _ => Arc::new(serde_json::Map::from_iter([(
            "type".to_string(),
            Value::String("object".to_string()),
        )])),
    };

    let output_schema = def.output_schema.as_ref().and_then(|s| match s {
        Value::Object(map) => Some(Arc::new(map.clone())),
        _ => None,
    });

    let annotations = def.annotations.as_ref().map(canonical_to_mcp_annotations);

    McpToolDef {
        name: Cow::Owned(def.name.clone()),
        title: def.title.clone(),
        description: Some(Cow::Owned(def.description.clone())),
        input_schema,
        output_schema,
        annotations,
        execution: None,
        icons: None,
        meta: None,
    }
}

/// Convert canonical [`ToolAnnotations`] to rmcp [`McpAnnotations`].
fn canonical_to_mcp_annotations(ann: &CanonicalAnnotations) -> McpAnnotations {
    McpAnnotations {
        title: None,
        read_only_hint: Some(ann.read_only),
        destructive_hint: Some(ann.destructive),
        idempotent_hint: Some(ann.idempotent),
        open_world_hint: Some(ann.open_world),
    }
}

/// Convert a canonical [`ToolResult`] to an rmcp [`CallToolResult`].
pub fn tool_result_to_call_result(result: &ToolResult) -> CallToolResult {
    let content: Vec<Content> = match &result.content {
        Some(blocks) => blocks.iter().map(canonical_content_to_mcp).collect(),
        None => {
            // Fall back to output field as text
            let text = match &result.output {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            vec![Content::text(text)]
        }
    };

    // Include structured content from the JSON output when it's an object
    let structured_content = match &result.output {
        Value::Object(_) => Some(result.output.clone()),
        _ => None,
    };

    CallToolResult {
        content,
        structured_content,
        is_error: Some(result.is_error),
        meta: None,
    }
}

/// Convert a single canonical [`ToolContent`] block to an rmcp [`Content`].
fn canonical_content_to_mcp(content: &ToolContent) -> Content {
    match content {
        ToolContent::Text { text } => Content::text(text.clone()),
        ToolContent::Image { data, mime_type } => Content::image(data.clone(), mime_type.clone()),
        ToolContent::Json { value } => {
            // json() returns Result; fall back to text serialization on error
            Content::json(value).unwrap_or_else(|_| Content::text(value.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::tool::ToolAnnotations as CanonicalAnnotations;
    use serde_json::json;

    #[test]
    fn definition_to_mcp_tool_full() {
        let def = ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            title: Some("Read File".into()),
            output_schema: Some(json!({"type": "string"})),
            annotations: Some(CanonicalAnnotations {
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
                requires_confirmation: false,
            }),
            category: Some("filesystem".into()),
            tags: vec!["fs".into()],
            timeout_secs: Some(30),
        };

        let mcp = definition_to_mcp_tool(&def);
        assert_eq!(mcp.name.as_ref(), "read_file");
        assert_eq!(mcp.description.as_deref(), Some("Read a file"));
        assert_eq!(mcp.title.as_deref(), Some("Read File"));
        assert!(mcp.output_schema.is_some());

        let ann = mcp.annotations.unwrap();
        assert_eq!(ann.read_only_hint, Some(true));
        assert_eq!(ann.destructive_hint, Some(false));
        assert_eq!(ann.idempotent_hint, Some(true));
        assert_eq!(ann.open_world_hint, Some(false));
    }

    #[test]
    fn definition_to_mcp_tool_minimal() {
        let def = ToolDefinition {
            name: "simple".into(),
            description: "A simple tool".into(),
            input_schema: json!({"type": "object"}),
            title: None,
            output_schema: None,
            annotations: None,
            category: None,
            tags: vec![],
            timeout_secs: None,
        };

        let mcp = definition_to_mcp_tool(&def);
        assert_eq!(mcp.name.as_ref(), "simple");
        assert!(mcp.annotations.is_none());
        assert!(mcp.output_schema.is_none());
        assert!(mcp.title.is_none());
    }

    #[test]
    fn definition_with_non_object_schema_wraps() {
        let def = ToolDefinition {
            name: "odd".into(),
            description: "Odd schema".into(),
            input_schema: json!("not an object"),
            title: None,
            output_schema: None,
            annotations: None,
            category: None,
            tags: vec![],
            timeout_secs: None,
        };

        let mcp = definition_to_mcp_tool(&def);
        let schema = mcp.input_schema.as_ref();
        assert_eq!(schema.get("type").unwrap(), "object");
    }

    #[test]
    fn tool_result_to_call_result_success_with_content() {
        let result = ToolResult {
            call_id: "c1".into(),
            tool_name: "test".into(),
            output: json!({"data": 42}),
            content: Some(vec![ToolContent::Text {
                text: "hello".into(),
            }]),
            is_error: false,
        };

        let mcp = tool_result_to_call_result(&result);
        assert_eq!(mcp.is_error, Some(false));
        assert_eq!(mcp.content.len(), 1);
        assert!(mcp.structured_content.is_some());
    }

    #[test]
    fn tool_result_to_call_result_error() {
        let result = ToolResult::error("c2", "bash", "permission denied");

        let mcp = tool_result_to_call_result(&result);
        assert_eq!(mcp.is_error, Some(true));
        assert!(!mcp.content.is_empty());
    }

    #[test]
    fn tool_result_to_call_result_no_content_falls_back_to_output() {
        let result = ToolResult {
            call_id: "c3".into(),
            tool_name: "test".into(),
            output: json!("plain text output"),
            content: None,
            is_error: false,
        };

        let mcp = tool_result_to_call_result(&result);
        assert_eq!(mcp.content.len(), 1);
        // No structured content for non-object output
        assert!(mcp.structured_content.is_none());
    }

    #[test]
    fn canonical_content_image_converts() {
        let content = ToolContent::Image {
            data: "base64data".into(),
            mime_type: "image/png".into(),
        };

        let mcp = canonical_content_to_mcp(&content);
        let raw = &mcp.raw;
        assert!(matches!(raw, rmcp::model::RawContent::Image(_)));
    }

    #[test]
    fn canonical_content_json_converts() {
        let content = ToolContent::Json {
            value: json!({"key": "value"}),
        };

        let mcp = canonical_content_to_mcp(&content);
        // JSON content should not be empty
        let raw = &mcp.raw;
        assert!(matches!(raw, rmcp::model::RawContent::Text(_)));
    }
}
