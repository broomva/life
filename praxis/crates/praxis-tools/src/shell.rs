//! Shell command execution tool.
//!
//! The `BashTool` wraps the sandbox's `CommandRunner` to execute
//! shell commands within policy constraints.

use aios_protocol::tool::{
    Tool, ToolAnnotations, ToolCall, ToolContext, ToolDefinition, ToolError, ToolResult,
};
use praxis_core::sandbox::{CommandRequest, CommandRunner, SandboxPolicy};
use serde_json::json;
use std::path::PathBuf;
use tracing::info;

/// Tool that executes bash commands within the sandbox.
pub struct BashTool {
    policy: SandboxPolicy,
    runner: Box<dyn CommandRunner>,
}

impl BashTool {
    pub fn new(policy: SandboxPolicy, runner: Box<dyn CommandRunner>) -> Self {
        Self { policy, runner }
    }
}

impl Tool for BashTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "bash".into(),
            description: "Executes a bash command in the sandbox.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command line to execute" },
                    "cwd": { "type": "string", "description": "Working directory (optional)" }
                },
                "required": ["command"]
            }),
            title: Some("Bash Command".into()),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                destructive: true,
                open_world: true,
                requires_confirmation: true,
                ..Default::default()
            }),
            category: Some("shell".into()),
            tags: vec!["shell".into(), "exec".into()],
            timeout_secs: Some(60),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let command_line = call
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'command' argument".into(),
            })?;

        let span = tracing::info_span!(
            "bash_execute",
            tool.name = "bash",
            call_id = %call.call_id,
            bash.exit_code = tracing::field::Empty,
        );
        let _guard = span.enter();

        let cwd = call
            .input
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.policy.workspace_root.clone());

        let request = CommandRequest {
            executable: "/bin/bash".into(),
            args: vec!["-c".into(), command_line.into()],
            cwd,
            env: vec![],
        };

        let result =
            self.runner
                .run(&self.policy, &request)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool_name: "bash".into(),
                    message: e.to_string(),
                })?;

        span.record("bash.exit_code", result.exit_code);
        info!(exit_code = result.exit_code, "bash command completed");

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: json!({
                "exit_code": result.exit_code,
                "stdout": result.stdout,
                "stderr": result.stderr
            }),
            content: None,
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::sandbox::NetworkPolicy;
    use aios_protocol::tool::{ToolCall, ToolContext};
    use praxis_core::sandbox::LocalCommandRunner;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    fn test_policy(dir: &std::path::Path) -> SandboxPolicy {
        SandboxPolicy {
            workspace_root: dir.to_path_buf(),
            shell_enabled: true,
            network: NetworkPolicy::Disabled,
            allowed_env: BTreeSet::new(),
            max_execution_ms: 5000,
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
        }
    }

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
    fn bash_executes_command() {
        let dir = TempDir::new().unwrap();
        let policy = test_policy(dir.path());
        let tool = BashTool::new(policy, Box::new(LocalCommandRunner::new()));
        let ctx = make_ctx();

        let call = make_call("bash", json!({"command": "echo hello"}));
        let result = tool.execute(&call, &ctx).unwrap();

        assert_eq!(result.output["exit_code"], 0);
        assert!(result.output["stdout"].as_str().unwrap().contains("hello"));
    }

    #[test]
    fn bash_shell_disabled_fails() {
        let dir = TempDir::new().unwrap();
        let mut policy = test_policy(dir.path());
        policy.shell_enabled = false;

        let tool = BashTool::new(policy, Box::new(LocalCommandRunner::new()));
        let ctx = make_ctx();

        let call = make_call("bash", json!({"command": "echo hello"}));
        let result = tool.execute(&call, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn bash_missing_command_fails() {
        let dir = TempDir::new().unwrap();
        let policy = test_policy(dir.path());
        let tool = BashTool::new(policy, Box::new(LocalCommandRunner::new()));
        let ctx = make_ctx();

        let call = make_call("bash", json!({}));
        let result = tool.execute(&call, &ctx);
        assert!(result.is_err());
    }
}
