//! Agent memory tools (file-based read/write).
//!
//! Memory is stored as markdown files keyed by name within
//! a configured memory directory.

use aios_protocol::tool::{
    Tool, ToolAnnotations, ToolCall, ToolContext, ToolDefinition, ToolError, ToolResult,
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tracing::debug;

// ── ReadMemoryTool ───────────────────────────────────────────────────

/// Reads the agent's persistent memory file by key.
pub struct ReadMemoryTool {
    memory_dir: PathBuf,
}

impl ReadMemoryTool {
    pub fn new(memory_dir: PathBuf) -> Self {
        Self { memory_dir }
    }
}

impl Tool for ReadMemoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_memory".into(),
            description: "Read the agent's persistent memory file by key.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Memory key (e.g. 'global', 'session', or custom name)" }
                },
                "required": ["key"]
            }),
            title: Some("Read Memory".into()),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            }),
            category: Some("memory".into()),
            tags: vec!["memory".into(), "read".into()],
            timeout_secs: Some(10),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let key = call
            .input
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'key' argument".into(),
            })?;

        let _span =
            tracing::debug_span!("read_memory", memory.operation = "read", memory.key = %key)
                .entered();

        validate_memory_key(key).map_err(|msg| ToolError::InvalidInput { message: msg })?;

        let file_path = self.memory_dir.join(format!("{key}.md"));

        if file_path.exists() {
            let content =
                fs::read_to_string(&file_path).map_err(|e| ToolError::ExecutionFailed {
                    tool_name: "read_memory".into(),
                    message: format!("Failed to read memory file: {e}"),
                })?;

            debug!(exists = true, bytes = content.len(), "memory read");

            Ok(ToolResult {
                call_id: call.call_id.clone(),
                tool_name: call.tool_name.clone(),
                output: json!({ "content": content, "exists": true, "path": file_path }),
                content: None,
                is_error: false,
                usage: None,
            })
        } else {
            debug!(exists = false, "memory key not found");

            Ok(ToolResult {
                call_id: call.call_id.clone(),
                tool_name: call.tool_name.clone(),
                output: json!({ "content": null, "exists": false, "path": file_path }),
                content: None,
                is_error: false,
                usage: None,
            })
        }
    }
}

// ── WriteMemoryTool ──────────────────────────────────────────────────

/// Writes to the agent's persistent memory file by key.
pub struct WriteMemoryTool {
    memory_dir: PathBuf,
}

impl WriteMemoryTool {
    pub fn new(memory_dir: PathBuf) -> Self {
        Self { memory_dir }
    }
}

impl Tool for WriteMemoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_memory".into(),
            description: "Write to the agent's persistent memory file by key.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Memory key (e.g. 'global', 'session', or custom name)" },
                    "content": { "type": "string", "description": "Markdown content to write" }
                },
                "required": ["key", "content"]
            }),
            title: Some("Write Memory".into()),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                destructive: true,
                ..Default::default()
            }),
            category: Some("memory".into()),
            tags: vec!["memory".into(), "write".into()],
            timeout_secs: Some(10),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let key = call
            .input
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'key' argument".into(),
            })?;

        let content = call
            .input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'content' argument".into(),
            })?;

        let _span = tracing::debug_span!(
            "write_memory",
            memory.operation = "write",
            memory.key = %key,
            memory.bytes = content.len(),
        )
        .entered();

        validate_memory_key(key).map_err(|msg| ToolError::InvalidInput { message: msg })?;

        fs::create_dir_all(&self.memory_dir).map_err(|e| ToolError::ExecutionFailed {
            tool_name: "write_memory".into(),
            message: format!("Failed to create memory directory: {e}"),
        })?;

        let file_path = self.memory_dir.join(format!("{key}.md"));

        fs::write(&file_path, content).map_err(|e| ToolError::ExecutionFailed {
            tool_name: "write_memory".into(),
            message: format!("Failed to write memory file: {e}"),
        })?;

        debug!(bytes = content.len(), "memory written");

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({ "success": true, "path": file_path }),
            content: None,
            is_error: false,
            usage: None,
        })
    }
}

/// Validate that a memory key is safe (alphanumeric, hyphens, underscores).
pub fn validate_memory_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("Memory key cannot be empty".into());
    }
    if key.len() > 64 {
        return Err("Memory key too long (max 64 characters)".into());
    }
    if !key
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "Memory key must contain only alphanumeric characters, hyphens, and underscores".into(),
        );
    }
    if key.starts_with('.') || key.contains("..") {
        return Err("Memory key cannot start with '.' or contain '..'".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::tool::{ToolCall, ToolContext};
    use tempfile::TempDir;

    fn make_ctx() -> ToolContext {
        ToolContext {
            run_id: "test-run".into(),
            session_id: "test".into(),
            iteration: 0,
            ..Default::default()
        }
    }

    fn make_call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            call_id: "test-call".into(),
            tool_name: name.into(),
            input,
            requested_capabilities: vec![],
        }
    }

    #[test]
    fn write_then_read_memory() {
        let dir = TempDir::new().unwrap();
        let write_tool = WriteMemoryTool::new(dir.path().to_path_buf());
        let read_tool = ReadMemoryTool::new(dir.path().to_path_buf());
        let ctx = make_ctx();

        let call = make_call(
            "write_memory",
            json!({"key": "test-notes", "content": "# Notes\nSome important info."}),
        );
        let result = write_tool.execute(&call, &ctx).unwrap();
        assert_eq!(result.output["success"], true);

        let call = make_call("read_memory", json!({"key": "test-notes"}));
        let result = read_tool.execute(&call, &ctx).unwrap();
        assert_eq!(result.output["exists"], true);
        assert_eq!(result.output["content"], "# Notes\nSome important info.");
    }

    #[test]
    fn read_nonexistent_memory() {
        let dir = TempDir::new().unwrap();
        let read_tool = ReadMemoryTool::new(dir.path().to_path_buf());
        let ctx = make_ctx();

        let call = make_call("read_memory", json!({"key": "nonexistent"}));
        let result = read_tool.execute(&call, &ctx).unwrap();
        assert_eq!(result.output["exists"], false);
        assert!(result.output["content"].is_null());
    }

    #[test]
    fn invalid_memory_key_rejected() {
        assert!(validate_memory_key("").is_err());
        assert!(validate_memory_key("../escape").is_err());
        assert!(validate_memory_key(".hidden").is_err());
        assert!(validate_memory_key("has spaces").is_err());
        assert!(validate_memory_key("has/slash").is_err());

        assert!(validate_memory_key("valid-key").is_ok());
        assert!(validate_memory_key("valid_key").is_ok());
        assert!(validate_memory_key("key123").is_ok());
    }

    #[test]
    fn write_creates_directory() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        assert!(!memory_dir.exists());

        let write_tool = WriteMemoryTool::new(memory_dir.clone());
        let ctx = make_ctx();

        let call = make_call(
            "write_memory",
            json!({"key": "auto-created", "content": "hello"}),
        );
        let result = write_tool.execute(&call, &ctx).unwrap();
        assert_eq!(result.output["success"], true);
        assert!(memory_dir.exists());
    }

    #[test]
    fn overwrite_memory() {
        let dir = TempDir::new().unwrap();
        let write_tool = WriteMemoryTool::new(dir.path().to_path_buf());
        let read_tool = ReadMemoryTool::new(dir.path().to_path_buf());
        let ctx = make_ctx();

        let call = make_call(
            "write_memory",
            json!({"key": "overwrite-test", "content": "version 1"}),
        );
        write_tool.execute(&call, &ctx).unwrap();

        let call = make_call(
            "write_memory",
            json!({"key": "overwrite-test", "content": "version 2"}),
        );
        write_tool.execute(&call, &ctx).unwrap();

        let call = make_call("read_memory", json!({"key": "overwrite-test"}));
        let result = read_tool.execute(&call, &ctx).unwrap();
        assert_eq!(result.output["content"], "version 2");
    }
}
