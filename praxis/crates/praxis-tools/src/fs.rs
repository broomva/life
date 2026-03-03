//! Filesystem tools: read, write, list, glob, grep.
//!
//! All operations are confined to the workspace root via [`FsPolicy`].

use crate::edit::render_hashed_content;
use aios_protocol::tool::{
    Tool, ToolAnnotations, ToolCall, ToolContext, ToolDefinition, ToolError, ToolResult,
};
use praxis_core::workspace::FsPolicy;
use regex::Regex;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

// ── ReadFileTool ─────────────────────────────────────────────────────

/// Reads a file and returns content with hashline tags for editing.
pub struct ReadFileTool {
    policy: FsPolicy,
}

impl ReadFileTool {
    pub fn new(policy: FsPolicy) -> Self {
        Self { policy }
    }
}

impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".into(),
            description: "Reads a file from the filesystem. Returns content with line numbers and hashes for editing.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" }
                },
                "required": ["path"]
            }),
            title: Some("Read File".into()),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            }),
            category: Some("filesystem".into()),
            tags: vec!["fs".into(), "read".into()],
            timeout_secs: Some(30),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = call
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'path' argument".into(),
            })?;

        let path = self
            .policy
            .resolve_existing(Path::new(path_str))
            .map_err(|e| ToolError::ExecutionFailed {
                tool_name: "read_file".into(),
                message: e.to_string(),
            })?;

        let content = fs::read_to_string(&path).map_err(|e| ToolError::ExecutionFailed {
            tool_name: "read_file".into(),
            message: format!("Failed to read file: {e}"),
        })?;

        let hashed_content = render_hashed_content(&content);

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({ "content": hashed_content, "path": path }),
            content: None,
            is_error: false,
        })
    }
}

// ── WriteFileTool ────────────────────────────────────────────────────

/// Writes content to a file, overwriting it completely.
pub struct WriteFileTool {
    policy: FsPolicy,
}

impl WriteFileTool {
    pub fn new(policy: FsPolicy) -> Self {
        Self { policy }
    }
}

impl Tool for WriteFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".into(),
            description: "Writes content to a file, overwriting it completely.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["path", "content"]
            }),
            title: Some("Write File".into()),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                destructive: true,
                ..Default::default()
            }),
            category: Some("filesystem".into()),
            tags: vec!["fs".into(), "write".into()],
            timeout_secs: Some(30),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = call
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'path' argument".into(),
            })?;

        let content = call
            .input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'content' argument".into(),
            })?;

        let path = self
            .policy
            .resolve_for_write(Path::new(path_str))
            .map_err(|e| ToolError::ExecutionFailed {
                tool_name: "write_file".into(),
                message: e.to_string(),
            })?;

        fs::write(&path, content).map_err(|e| ToolError::ExecutionFailed {
            tool_name: "write_file".into(),
            message: format!("Failed to write file: {e}"),
        })?;

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({ "success": true, "path": path }),
            content: None,
            is_error: false,
        })
    }
}

// ── ListDirTool ──────────────────────────────────────────────────────

/// Lists contents of a directory.
pub struct ListDirTool {
    policy: FsPolicy,
}

impl ListDirTool {
    pub fn new(policy: FsPolicy) -> Self {
        Self { policy }
    }
}

impl Tool for ListDirTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_dir".into(),
            description: "Lists contents of a directory.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the directory" }
                },
                "required": ["path"]
            }),
            title: Some("List Directory".into()),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            }),
            category: Some("filesystem".into()),
            tags: vec!["fs".into(), "list".into()],
            timeout_secs: Some(30),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = call
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'path' argument".into(),
            })?;

        let path = self
            .policy
            .resolve_existing(Path::new(path_str))
            .map_err(|e| ToolError::ExecutionFailed {
                tool_name: "list_dir".into(),
                message: e.to_string(),
            })?;

        let entries = fs::read_dir(&path)
            .map_err(|e| ToolError::ExecutionFailed {
                tool_name: "list_dir".into(),
                message: format!("Failed to read dir: {e}"),
            })?
            .filter_map(|entry| {
                entry.ok().map(|e: std::fs::DirEntry| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let kind = if e.path().is_dir() { "dir" } else { "file" };
                    json!({ "name": name, "kind": kind })
                })
            })
            .collect::<Vec<_>>();

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({ "entries": entries, "path": path }),
            content: None,
            is_error: false,
        })
    }
}

// ── GlobTool ─────────────────────────────────────────────────────────

/// Searches for files matching a glob pattern within the workspace.
pub struct GlobTool {
    policy: FsPolicy,
}

impl GlobTool {
    pub fn new(policy: FsPolicy) -> Self {
        Self { policy }
    }
}

impl Tool for GlobTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "glob".into(),
            description: "Search for files matching a glob pattern within the workspace.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern (e.g. **/*.rs)" },
                    "path": { "type": "string", "description": "Base directory (optional, defaults to workspace root)" }
                },
                "required": ["pattern"]
            }),
            title: Some("Glob Search".into()),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            }),
            category: Some("filesystem".into()),
            tags: vec!["fs".into(), "search".into()],
            timeout_secs: Some(30),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let pattern = call
            .input
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'pattern' argument".into(),
            })?;

        let base_dir = call
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.policy.workspace_root().to_path_buf());

        let base_dir = self
            .policy
            .resolve_existing(Path::new(&base_dir))
            .map_err(|e| ToolError::ExecutionFailed {
                tool_name: "glob".into(),
                message: e.to_string(),
            })?;

        let full_pattern = base_dir.join(pattern).display().to_string();

        let matches: Vec<String> = glob::glob(&full_pattern)
            .map_err(|e| ToolError::ExecutionFailed {
                tool_name: "glob".into(),
                message: format!("Invalid glob pattern: {e}"),
            })?
            .filter_map(Result::ok)
            .filter(|path| self.policy.resolve_existing(path).is_ok())
            .map(|path| {
                path.strip_prefix(self.policy.workspace_root())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string())
            })
            .collect();

        let count = matches.len();

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({ "matches": matches, "count": count }),
            content: None,
            is_error: false,
        })
    }
}

// ── GrepTool ─────────────────────────────────────────────────────────

/// Searches file contents for a regex pattern within the workspace.
pub struct GrepTool {
    policy: FsPolicy,
}

impl GrepTool {
    pub fn new(policy: FsPolicy) -> Self {
        Self { policy }
    }
}

impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "grep".into(),
            description: "Search file contents for a regex pattern within the workspace.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "Directory to search (optional, defaults to workspace root)" },
                    "glob": { "type": "string", "description": "File glob filter (e.g. *.rs)" },
                    "max_matches": { "type": "integer", "description": "Maximum number of matches to return (default 100)" }
                },
                "required": ["pattern"]
            }),
            title: Some("Grep Search".into()),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            }),
            category: Some("filesystem".into()),
            tags: vec!["fs".into(), "search".into()],
            timeout_secs: Some(60),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let pattern_str = call
            .input
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing or invalid 'pattern' argument".into(),
            })?;

        let regex = Regex::new(pattern_str).map_err(|e| ToolError::ExecutionFailed {
            tool_name: "grep".into(),
            message: format!("Invalid regex pattern: {e}"),
        })?;

        let base_dir = call
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.policy.workspace_root().to_path_buf());

        let base_dir = self
            .policy
            .resolve_existing(Path::new(&base_dir))
            .map_err(|e| ToolError::ExecutionFailed {
                tool_name: "grep".into(),
                message: e.to_string(),
            })?;

        let glob_filter = call.input.get("glob").and_then(|v| v.as_str());

        let max_matches = call
            .input
            .get("max_matches")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(100) as usize;

        let mut matches = Vec::new();

        for entry in walkdir::WalkDir::new(&base_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();

            // Apply glob filter if specified
            if let Some(glob_pat) = glob_filter {
                let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                let glob_pattern =
                    glob::Pattern::new(glob_pat).map_err(|e| ToolError::ExecutionFailed {
                        tool_name: "grep".into(),
                        message: format!("Invalid glob filter: {e}"),
                    })?;
                if !glob_pattern.matches(&file_name) {
                    continue;
                }
            }

            // Skip large files (>10MB)
            if let Ok(metadata) = path.metadata() {
                if metadata.len() > 10 * 1024 * 1024 {
                    continue;
                }
            }

            let Ok(file) = fs::File::open(path) else {
                continue;
            };

            let reader = BufReader::new(file);
            for (line_no, line) in reader.lines().enumerate() {
                let Ok(line) = line else {
                    continue;
                };

                if regex.is_match(&line) {
                    let rel_path = path
                        .strip_prefix(self.policy.workspace_root())
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| path.display().to_string());

                    matches.push(json!({
                        "file": rel_path,
                        "line": line_no + 1,
                        "text": line
                    }));

                    if matches.len() >= max_matches {
                        break;
                    }
                }
            }

            if matches.len() >= max_matches {
                break;
            }
        }

        let count = matches.len();

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({ "matches": matches, "count": count }),
            content: None,
            is_error: false,
        })
    }
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
    fn read_file_returns_hashed_content() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello\nworld").unwrap();

        let policy = FsPolicy::new(dir.path());
        let tool = ReadFileTool::new(policy);
        let ctx = make_ctx();

        let call = make_call("read_file", json!({"path": "test.txt"}));
        let result = tool.execute(&call, &ctx).unwrap();

        let content = result.output["content"].as_str().unwrap();
        assert!(content.contains("hello"));
        assert!(content.contains("world"));
        assert!(content.contains("   1 ")); // line number
    }

    #[test]
    fn read_file_outside_workspace_fails() {
        let dir = TempDir::new().unwrap();
        let policy = FsPolicy::new(dir.path());
        let tool = ReadFileTool::new(policy);
        let ctx = make_ctx();

        let call = make_call("read_file", json!({"path": "/etc/passwd"}));
        let result = tool.execute(&call, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn write_then_read_file() {
        let dir = TempDir::new().unwrap();
        let policy = FsPolicy::new(dir.path());
        let write_tool = WriteFileTool::new(policy.clone());
        let read_tool = ReadFileTool::new(policy);
        let ctx = make_ctx();

        let call = make_call(
            "write_file",
            json!({"path": "new.txt", "content": "created"}),
        );
        let result = write_tool.execute(&call, &ctx).unwrap();
        assert_eq!(result.output["success"], true);

        let call = make_call("read_file", json!({"path": "new.txt"}));
        let result = read_tool.execute(&call, &ctx).unwrap();
        let content = result.output["content"].as_str().unwrap();
        assert!(content.contains("created"));
    }

    #[test]
    fn list_dir_shows_entries() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let policy = FsPolicy::new(dir.path());
        let tool = ListDirTool::new(policy);
        let ctx = make_ctx();

        let call = make_call("list_dir", json!({"path": "."}));
        let result = tool.execute(&call, &ctx).unwrap();

        let entries = result.output["entries"].as_array().unwrap();
        assert!(entries.len() >= 2);

        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"a.txt"));
        assert!(names.contains(&"sub"));
    }

    #[test]
    fn glob_finds_matching_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.rs"), "").unwrap();
        std::fs::write(dir.path().join("c.txt"), "").unwrap();

        let policy = FsPolicy::new(dir.path());
        let tool = GlobTool::new(policy);
        let ctx = make_ctx();

        let call = make_call("glob", json!({"pattern": "*.rs"}));
        let result = tool.execute(&call, &ctx).unwrap();

        assert_eq!(result.output["count"], 2);
        let matches = result.output["matches"].as_array().unwrap();
        assert!(matches.iter().any(|m| m.as_str().unwrap().contains("a.rs")));
        assert!(matches.iter().any(|m| m.as_str().unwrap().contains("b.rs")));
    }

    #[test]
    fn grep_finds_matching_lines() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("code.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("other.txt"), "no match here\n").unwrap();

        let policy = FsPolicy::new(dir.path());
        let tool = GrepTool::new(policy);
        let ctx = make_ctx();

        let call = make_call("grep", json!({"pattern": "println"}));
        let result = tool.execute(&call, &ctx).unwrap();

        assert_eq!(result.output["count"], 1);
        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(matches[0]["line"], 2);
        assert!(matches[0]["text"].as_str().unwrap().contains("println"));
    }

    #[test]
    fn grep_with_glob_filter() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn hello() {}\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "fn hello() {}\n").unwrap();

        let policy = FsPolicy::new(dir.path());
        let tool = GrepTool::new(policy);
        let ctx = make_ctx();

        let call = make_call("grep", json!({"pattern": "hello", "glob": "*.rs"}));
        let result = tool.execute(&call, &ctx).unwrap();

        assert_eq!(result.output["count"], 1);
        let matches = result.output["matches"].as_array().unwrap();
        assert!(matches[0]["file"].as_str().unwrap().contains("a.rs"));
    }

    #[test]
    fn grep_respects_max_matches() {
        let dir = TempDir::new().unwrap();
        let content: String = (0..50).map(|i| format!("match line {i}\n")).collect();
        std::fs::write(dir.path().join("many.txt"), content).unwrap();

        let policy = FsPolicy::new(dir.path());
        let tool = GrepTool::new(policy);
        let ctx = make_ctx();

        let call = make_call("grep", json!({"pattern": "match", "max_matches": 5}));
        let result = tool.execute(&call, &ctx).unwrap();

        assert_eq!(result.output["count"], 5);
    }
}
