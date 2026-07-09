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
use std::time::Instant;
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

    fn execute(&self, call: &ToolCall, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let command_line = call
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'command' argument".into(),
            })?;

        let span = tracing::info_span!(
            "praxis.shell.execute",
            "praxis.command" = %command_line,
            "praxis.exit_code" = tracing::field::Empty,
            "praxis.duration_ms" = tracing::field::Empty,
        );
        let _guard = span.enter();
        let start = Instant::now();

        // BRO-1491: when the kernel threaded a per-session workspace root,
        // rebase the sandbox boundary there so shell commands run inside — and
        // cannot escape to — the session workspace. Otherwise use the
        // construction-time (boot) policy.
        let policy = match ctx.workspace_root.as_deref().filter(|r| !r.is_empty()) {
            Some(root) => SandboxPolicy {
                workspace_root: PathBuf::from(root),
                ..self.policy.clone()
            },
            None => self.policy.clone(),
        };

        let cwd = call
            .input
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| policy.workspace_root.clone());

        let request = CommandRequest {
            executable: "/bin/bash".into(),
            args: vec!["-c".into(), command_line.into()],
            cwd,
            env: vec![],
        };

        let result =
            self.runner
                .run(&policy, &request)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool_name: "bash".into(),
                    message: e.to_string(),
                })?;

        let duration_ms = start.elapsed().as_millis() as u64;
        span.record("praxis.exit_code", result.exit_code);
        span.record("praxis.duration_ms", duration_ms);
        info!(
            exit_code = result.exit_code,
            duration_ms, "bash command completed"
        );

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
            usage: None,
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
    fn bash_cwd_follows_session_workspace_root() {
        // BRO-1491: the boot policy roots at `boot`, but the call carries a
        // per-session workspace root — the command must run there.
        let dir = TempDir::new().unwrap();
        let boot = dir.path().join("boot");
        let session = dir.path().join("sessions/s1");
        std::fs::create_dir_all(&boot).unwrap();
        std::fs::create_dir_all(&session).unwrap();

        let policy = test_policy(&boot);
        let tool = BashTool::new(policy, Box::new(LocalCommandRunner::new()));
        let ctx = ToolContext {
            workspace_root: Some(session.to_string_lossy().into_owned()),
            ..make_ctx()
        };

        let call = make_call("bash", json!({"command": "pwd"}));
        let result = tool.execute(&call, &ctx).unwrap();
        assert_eq!(result.output["exit_code"], 0);
        let stdout = result.output["stdout"].as_str().unwrap();
        let canonical_session = session.canonicalize().unwrap();
        assert!(
            stdout
                .trim()
                .ends_with(canonical_session.file_name().unwrap().to_str().unwrap()),
            "pwd `{}` should be the session workspace `{}`",
            stdout.trim(),
            canonical_session.display()
        );
    }

    #[test]
    fn bash_scoped_cwd_outside_session_rejected() {
        // An explicit cwd escaping the session boundary is rejected.
        let dir = TempDir::new().unwrap();
        let boot = dir.path().join("boot");
        let session = dir.path().join("sessions/s1");
        std::fs::create_dir_all(&boot).unwrap();
        std::fs::create_dir_all(&session).unwrap();

        let tool = BashTool::new(test_policy(&boot), Box::new(LocalCommandRunner::new()));
        let ctx = ToolContext {
            workspace_root: Some(session.to_string_lossy().into_owned()),
            ..make_ctx()
        };

        // `boot` is outside the session workspace → cwd validation rejects it.
        let call = make_call(
            "bash",
            json!({"command": "pwd", "cwd": boot.to_string_lossy()}),
        );
        assert!(tool.execute(&call, &ctx).is_err());
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
