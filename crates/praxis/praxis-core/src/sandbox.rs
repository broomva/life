//! Sandbox policy and command execution.
//!
//! Provides the `SandboxPolicy` configuration and `CommandRunner` trait
//! for executing commands within policy constraints.

use crate::error::{PraxisError, PraxisResult};
use aios_protocol::sandbox::NetworkPolicy;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Security policy for sandboxed command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Filesystem root that all operations are confined to.
    pub workspace_root: PathBuf,
    /// Whether shell execution is enabled.
    #[serde(default = "default_true")]
    pub shell_enabled: bool,
    /// Network access policy.
    #[serde(default)]
    pub network: NetworkPolicy,
    /// Environment variables allowed to pass through.
    #[serde(default)]
    pub allowed_env: BTreeSet<String>,
    /// Maximum execution time in milliseconds.
    #[serde(default = "default_max_execution_ms")]
    pub max_execution_ms: u64,
    /// Maximum stdout size in bytes.
    #[serde(default = "default_max_output_bytes")]
    pub max_stdout_bytes: usize,
    /// Maximum stderr size in bytes.
    #[serde(default = "default_max_output_bytes")]
    pub max_stderr_bytes: usize,
}

fn default_true() -> bool {
    true
}

fn default_max_execution_ms() -> u64 {
    60_000
}

fn default_max_output_bytes() -> usize {
    256 * 1024
}

impl SandboxPolicy {
    /// Create a new policy with the given workspace root.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            shell_enabled: true,
            network: NetworkPolicy::Disabled,
            allowed_env: BTreeSet::new(),
            max_execution_ms: default_max_execution_ms(),
            max_stdout_bytes: default_max_output_bytes(),
            max_stderr_bytes: default_max_output_bytes(),
        }
    }
}

/// A command to execute.
#[derive(Debug, Clone)]
pub struct CommandRequest {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Result of a command execution.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Trait for running commands within a sandbox policy.
pub trait CommandRunner: Send + Sync {
    fn run(&self, policy: &SandboxPolicy, request: &CommandRequest) -> PraxisResult<CommandResult>;
}

/// Local command runner that enforces sandbox policy via process controls.
#[derive(Debug, Default)]
pub struct LocalCommandRunner;

impl LocalCommandRunner {
    pub fn new() -> Self {
        Self
    }

    /// Validate that cwd is within the workspace boundary.
    fn validate_cwd(policy: &SandboxPolicy, cwd: &std::path::Path) -> PraxisResult<PathBuf> {
        // Canonicalize both to handle symlinks
        let canonical_workspace = policy.workspace_root.canonicalize().map_err(|e| {
            PraxisError::WorkspaceViolation(format!("cannot resolve workspace: {e}"))
        })?;
        let canonical_cwd = cwd
            .canonicalize()
            .map_err(|e| PraxisError::WorkspaceViolation(format!("cannot resolve cwd: {e}")))?;

        if !canonical_cwd.starts_with(&canonical_workspace) {
            return Err(PraxisError::PathOutsideWorkspace {
                path: cwd.display().to_string(),
            });
        }
        Ok(canonical_cwd)
    }

    /// Filter environment variables through the allow list.
    fn filter_env(policy: &SandboxPolicy, requested: &[(String, String)]) -> Vec<(String, String)> {
        requested
            .iter()
            .filter(|(key, _)| policy.allowed_env.contains(key))
            .cloned()
            .collect()
    }

    /// Truncate output to the maximum allowed size.
    fn truncate(output: Vec<u8>, max_bytes: usize) -> String {
        let mut bytes = output;
        if bytes.len() > max_bytes {
            bytes.truncate(max_bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl CommandRunner for LocalCommandRunner {
    fn run(&self, policy: &SandboxPolicy, request: &CommandRequest) -> PraxisResult<CommandResult> {
        let span = tracing::info_span!(
            "sandbox_run",
            sandbox.executable = %request.executable,
            sandbox.exit_code = tracing::field::Empty,
            sandbox.duration_ms = tracing::field::Empty,
        );
        let _guard = span.enter();

        if !policy.shell_enabled {
            return Err(PraxisError::Sandbox(
                "shell execution is disabled by policy".into(),
            ));
        }

        let canonical_cwd = Self::validate_cwd(policy, &request.cwd)?;
        let filtered_env = Self::filter_env(policy, &request.env);

        debug!(
            executable = %request.executable,
            cwd = %canonical_cwd.display(),
            "executing command"
        );

        let start = std::time::Instant::now();

        let mut cmd = std::process::Command::new(&request.executable);
        cmd.args(&request.args);
        cmd.current_dir(&canonical_cwd);
        cmd.env_clear();
        for (key, val) in &filtered_env {
            cmd.env(key, val);
        }

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| PraxisError::CommandFailed(format!("spawn failed: {e}")))?;

        let timeout = std::time::Duration::from_millis(policy.max_execution_ms);
        let result = match wait_timeout::ChildExt::wait_timeout(&mut child, timeout) {
            Ok(Some(status)) => {
                let stdout =
                    Self::truncate(read_pipe(child.stdout.take()), policy.max_stdout_bytes);
                let stderr =
                    Self::truncate(read_pipe(child.stderr.take()), policy.max_stderr_bytes);

                let exit_code = status.code().unwrap_or(-1);
                span.record("sandbox.exit_code", exit_code);
                info!(exit_code, "command completed");

                Ok(CommandResult {
                    exit_code,
                    stdout,
                    stderr,
                })
            }
            Ok(None) => {
                // Timed out — kill the child
                warn!(
                    executable = %request.executable,
                    timeout_ms = policy.max_execution_ms,
                    "command timed out, killing"
                );
                let _ = child.kill();
                let _ = child.wait();
                span.record("sandbox.exit_code", -1);
                Ok(CommandResult {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("command timed out after {}ms", policy.max_execution_ms),
                })
            }
            Err(e) => Err(PraxisError::CommandFailed(format!("wait failed: {e}"))),
        };

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("sandbox.duration_ms", elapsed);

        result
    }
}

/// Read all bytes from an optional pipe.
fn read_pipe(pipe: Option<impl std::io::Read>) -> Vec<u8> {
    let Some(mut pipe) = pipe else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy(dir: &std::path::Path) -> SandboxPolicy {
        SandboxPolicy {
            workspace_root: dir.to_path_buf(),
            shell_enabled: true,
            network: NetworkPolicy::Disabled,
            allowed_env: BTreeSet::from(["PATH".to_string(), "HOME".to_string()]),
            max_execution_ms: 5000,
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
        }
    }

    #[test]
    fn shell_disabled_rejects_execution() {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = test_policy(dir.path());
        policy.shell_enabled = false;

        let runner = LocalCommandRunner::new();
        let request = CommandRequest {
            executable: "echo".into(),
            args: vec!["hello".into()],
            cwd: dir.path().to_path_buf(),
            env: vec![],
        };

        let err = runner.run(&policy, &request).unwrap_err();
        assert!(err.to_string().contains("disabled by policy"));
    }

    #[test]
    fn cwd_outside_workspace_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let policy = test_policy(dir.path());

        let err =
            LocalCommandRunner::validate_cwd(&policy, std::path::Path::new("/tmp")).unwrap_err();
        assert!(matches!(err, PraxisError::PathOutsideWorkspace { .. }));
    }

    #[test]
    fn env_filtering() {
        let policy = SandboxPolicy {
            workspace_root: PathBuf::from("/tmp"),
            shell_enabled: true,
            network: NetworkPolicy::Disabled,
            allowed_env: BTreeSet::from(["PATH".to_string()]),
            max_execution_ms: 5000,
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
        };

        let env = vec![
            ("PATH".into(), "/usr/bin".into()),
            ("SECRET".into(), "hidden".into()),
        ];
        let filtered = LocalCommandRunner::filter_env(&policy, &env);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0, "PATH");
    }

    #[test]
    fn output_truncation() {
        let long = vec![b'x'; 2000];
        let truncated = LocalCommandRunner::truncate(long, 100);
        assert_eq!(truncated.len(), 100);
    }

    #[test]
    fn execute_echo_command() {
        let dir = tempfile::tempdir().unwrap();
        let policy = test_policy(dir.path());
        let runner = LocalCommandRunner::new();

        let request = CommandRequest {
            executable: "echo".into(),
            args: vec!["hello".into()],
            cwd: dir.path().to_path_buf(),
            env: vec![],
        };

        let result = runner.run(&policy, &request).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[test]
    fn timeout_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = test_policy(dir.path());
        policy.max_execution_ms = 100; // 100ms timeout

        let runner = LocalCommandRunner::new();
        let request = CommandRequest {
            executable: "sleep".into(),
            args: vec!["10".into()],
            cwd: dir.path().to_path_buf(),
            env: vec![],
        };

        let result = runner.run(&policy, &request).unwrap();
        assert_eq!(result.exit_code, -1);
        assert!(result.stderr.contains("timed out"));
    }
}
