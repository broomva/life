//! Integration tests: real Praxis tools exposed through the MCP server.
//!
//! Tests the full flow: register tools → build MCP server → call_tool/list_tools.

use aios_protocol::tool::ToolRegistry;
use praxis_core::local_fs::LocalFs;
use praxis_core::workspace::FsPolicy;
use praxis_mcp_bridge::server::PraxisMcpServer;
use praxis_tools::fs::{GlobTool, GrepTool, ListDirTool, ReadFileTool, WriteFileTool};
use praxis_tools::memory::{ReadMemoryTool, WriteMemoryTool};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// Build a PraxisMcpServer with all filesystem and memory tools.
fn server_with_real_tools(workspace: &std::path::Path) -> PraxisMcpServer {
    let policy = FsPolicy::new(workspace);
    let fs = Arc::new(LocalFs::new(policy));
    let memory_dir = workspace.join(".memory");
    std::fs::create_dir_all(&memory_dir).unwrap();

    let mut registry = ToolRegistry::default();
    registry.register(ReadFileTool::new(fs.clone()));
    registry.register(WriteFileTool::new(fs.clone()));
    registry.register(ListDirTool::new(fs.clone()));
    registry.register(GlobTool::new(fs.clone()));
    registry.register(GrepTool::new(fs));
    registry.register(ReadMemoryTool::new(memory_dir.clone()));
    registry.register(WriteMemoryTool::new(memory_dir));

    PraxisMcpServer::new(registry)
}

/// Helper to call a tool on the server synchronously (mimicking MCP call_tool).
///
/// Exercises the same code path as `ServerHandler::call_tool` but without
/// requiring a full transport/peer setup.
fn call_tool(
    server: &PraxisMcpServer,
    tool_name: &str,
    args: serde_json::Value,
) -> rmcp::model::CallToolResult {
    let call = aios_protocol::tool::ToolCall {
        call_id: "test-call".to_string(),
        tool_name: tool_name.to_string(),
        input: args,
        requested_capabilities: vec![],
    };
    let ctx = aios_protocol::tool::ToolContext {
        run_id: "test-run".to_string(),
        session_id: "test-session".to_string(),
        iteration: 0,
        ..Default::default()
    };

    let registry_tool = server.registry().get(tool_name).unwrap();
    let result = registry_tool.execute(&call, &ctx).unwrap();
    praxis_mcp_bridge::convert::tool_result_to_call_result(&result)
}

#[test]
fn list_tools_returns_all_praxis_tools() {
    let tmp = TempDir::new().unwrap();
    let server = server_with_real_tools(tmp.path());
    let tools = server.mcp_tools();

    // Should have 7 tools: read_file, write_file, list_dir, glob, grep, read_memory, write_memory
    assert_eq!(tools.len(), 7);

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"read_file"), "missing read_file");
    assert!(names.contains(&"write_file"), "missing write_file");
    assert!(names.contains(&"list_dir"), "missing list_dir");
    assert!(names.contains(&"glob"), "missing glob");
    assert!(names.contains(&"grep"), "missing grep");
    assert!(names.contains(&"read_memory"), "missing read_memory");
    assert!(names.contains(&"write_memory"), "missing write_memory");
}

#[test]
fn tool_schemas_are_valid_json_objects() {
    let tmp = TempDir::new().unwrap();
    let server = server_with_real_tools(tmp.path());

    for tool in server.mcp_tools() {
        let schema = tool.input_schema.as_ref();
        assert!(
            schema.contains_key("type"),
            "tool {} input_schema missing 'type'",
            tool.name
        );
        assert_eq!(
            schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "tool {} input_schema type must be 'object'",
            tool.name
        );
    }
}

#[test]
fn write_and_read_file_through_mcp() {
    let tmp = TempDir::new().unwrap();
    let server = server_with_real_tools(tmp.path());

    // Write a file
    let write_result = call_tool(
        &server,
        "write_file",
        json!({
            "path": "test.txt",
            "content": "hello from MCP"
        }),
    );
    assert_ne!(write_result.is_error, Some(true));

    // Read it back
    let read_result = call_tool(&server, "read_file", json!({"path": "test.txt"}));
    assert_ne!(read_result.is_error, Some(true));

    // Verify content appears in the response
    let text = read_result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert!(
        text.contains("hello from MCP"),
        "read_file should return the written content, got: {text}"
    );
}

#[test]
fn list_dir_through_mcp() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "aaa").unwrap();
    std::fs::write(tmp.path().join("b.txt"), "bbb").unwrap();

    let server = server_with_real_tools(tmp.path());
    let result = call_tool(&server, "list_dir", json!({"path": "."}));
    assert_ne!(result.is_error, Some(true));

    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert!(text.contains("a.txt"), "list_dir should show a.txt");
    assert!(text.contains("b.txt"), "list_dir should show b.txt");
}

#[test]
fn glob_finds_files_through_mcp() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("foo.rs"), "fn main() {}").unwrap();
    std::fs::write(tmp.path().join("bar.txt"), "text").unwrap();

    let server = server_with_real_tools(tmp.path());
    let result = call_tool(&server, "glob", json!({"pattern": "**/*.rs"}));
    assert_ne!(result.is_error, Some(true));

    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert!(text.contains("foo.rs"), "glob should find foo.rs");
    assert!(!text.contains("bar.txt"), "glob should not find bar.txt");
}

#[test]
fn grep_searches_content_through_mcp() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("code.rs"),
        "fn hello_world() {}\nfn goodbye() {}",
    )
    .unwrap();

    let server = server_with_real_tools(tmp.path());
    let result = call_tool(&server, "grep", json!({"pattern": "hello", "glob": "*.rs"}));
    assert_ne!(result.is_error, Some(true));

    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert!(
        text.contains("hello_world"),
        "grep should find hello_world match"
    );
}

#[test]
fn memory_write_and_read_through_mcp() {
    let tmp = TempDir::new().unwrap();
    let server = server_with_real_tools(tmp.path());

    // Write memory
    let write_result = call_tool(
        &server,
        "write_memory",
        json!({"key": "test-note", "content": "remember this"}),
    );
    assert_ne!(write_result.is_error, Some(true));

    // Read it back
    let read_result = call_tool(&server, "read_memory", json!({"key": "test-note"}));
    assert_ne!(read_result.is_error, Some(true));

    let text = read_result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert!(
        text.contains("remember this"),
        "read_memory should return written content"
    );
}

#[test]
fn server_info_describes_praxis() {
    let tmp = TempDir::new().unwrap();
    let server = server_with_real_tools(tmp.path());
    let info = server.get_info();

    assert_eq!(info.server_info.name, "praxis");
    assert!(info.capabilities.tools.is_some());
    assert_eq!(
        info.server_info.title.as_deref(),
        Some("Praxis Tool Engine")
    );
}

#[test]
fn tool_annotations_propagate_correctly() {
    let tmp = TempDir::new().unwrap();
    let server = server_with_real_tools(tmp.path());
    let tools = server.mcp_tools();

    // read_file should be read-only
    let read_tool = tools.iter().find(|t| t.name == "read_file").unwrap();
    let ann = read_tool.annotations.as_ref().unwrap();
    assert_eq!(ann.read_only_hint, Some(true));
    assert_eq!(ann.destructive_hint, Some(false));

    // write_file should be destructive
    let write_tool = tools.iter().find(|t| t.name == "write_file").unwrap();
    let ann = write_tool.annotations.as_ref().unwrap();
    assert_eq!(ann.destructive_hint, Some(true));
    assert_eq!(ann.read_only_hint, Some(false));
}
