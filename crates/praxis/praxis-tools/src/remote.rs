// Phase 0 transitional: invokes `arcan_sandbox::SandboxProvider` methods via
// the blanket `impl<T: HypervisorBackend> SandboxProvider for T`. Migration
// to direct `HypervisorBackend` is deferred to a follow-up phase.
#![allow(deprecated)]

//! Remote command execution via [`arcan_sandbox::SandboxProvider`].
//!
//! [`RemoteCommandRunner`] bridges the synchronous [`CommandRunner`] trait
//! (consumed by [`crate::shell::BashTool`]) to the async
//! [`arcan_sandbox::SandboxProvider`] backend. The translation layer maps
//! Praxis sandbox policy constraints onto the provider's [`ExecRequest`] and
//! converts the resulting [`ExecResult`] back into Praxis-native types.
//!
//! # Design note
//!
//! The sync↔async bridge uses `tokio::task::block_in_place` when running
//! inside a multi-threaded Tokio runtime so that the async event-loop thread
//! is not blocked. When no runtime is present a temporary single-threaded
//! runtime is created for the duration of the call.
//!
//! This module is the implementation for ticket BRO-247.

use std::collections::HashMap;
use std::sync::Arc;

use praxis_core::error::{PraxisError, PraxisResult};
use praxis_core::sandbox::{CommandRequest, CommandResult, CommandRunner, SandboxPolicy};
use tracing::{error, info};

// ── Async↔sync bridge ────────────────────────────────────────────────────────

/// Drive an async future to completion from synchronous code.
///
/// * If we are already inside a Tokio multi-thread runtime, this uses
///   [`tokio::task::block_in_place`] so the current OS thread may block
///   without starving the async pool.
/// * Otherwise a new single-threaded runtime is created just for this call.
fn block_on_async<F, T>(f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // Already inside a Tokio runtime — block the current OS thread
            // using block_in_place so the scheduler can park other tasks.
            tokio::task::block_in_place(|| handle.block_on(f))
        }
        Err(_) => {
            // No runtime context — spin one up for this call only.
            tokio::runtime::Runtime::new()
                .expect("failed to create tokio runtime")
                .block_on(f)
        }
    }
}

// ── RemoteCommandRunner ───────────────────────────────────────────────────────

/// A [`CommandRunner`] implementation that dispatches commands to a remote
/// [`arcan_sandbox::SandboxProvider`] backend (e.g. Vercel Sandbox, E2B,
/// local container).
///
/// Used by [`crate::shell::BashTool`] when the session tier requires
/// container-level isolation (see BRO-247). The sandbox lifecycle is managed
/// externally by `SandboxRouter` in `arcand`; `RemoteCommandRunner` receives
/// an already-running sandbox ID.
pub struct RemoteCommandRunner {
    provider: Arc<dyn arcan_sandbox::SandboxProvider>,
    sandbox_id: arcan_sandbox::SandboxId,
}

impl RemoteCommandRunner {
    /// Create a new runner that dispatches to `provider` targeting the sandbox
    /// identified by `sandbox_id`.
    ///
    /// The caller is responsible for ensuring the sandbox is in `Running`
    /// state before commands are submitted.
    pub fn new(
        provider: Arc<dyn arcan_sandbox::SandboxProvider>,
        sandbox_id: arcan_sandbox::SandboxId,
    ) -> Self {
        Self {
            provider,
            sandbox_id,
        }
    }
}

impl CommandRunner for RemoteCommandRunner {
    fn run(&self, policy: &SandboxPolicy, request: &CommandRequest) -> PraxisResult<CommandResult> {
        if !policy.shell_enabled {
            return Err(PraxisError::Sandbox(
                "shell execution disabled by policy".into(),
            ));
        }

        let exec_req = to_exec_request(policy, request);
        let provider = Arc::clone(&self.provider);
        let sandbox_id = self.sandbox_id.clone();

        info!(
            sandbox_id = %sandbox_id,
            executable = %request.executable,
            "dispatching command to remote sandbox"
        );

        let result = block_on_async(async move { provider.run(&sandbox_id, exec_req).await })
            .map_err(|e| {
                error!(error = %e, "remote sandbox exec failed");
                PraxisError::CommandFailed(e.to_string())
            })?;

        info!(
            exit_code = result.exit_code,
            duration_ms = result.duration_ms,
            "remote sandbox exec completed"
        );

        from_exec_result(result, policy)
    }
}

// ── Translation helpers ───────────────────────────────────────────────────────

/// Translate a Praxis [`CommandRequest`] + [`SandboxPolicy`] into an
/// [`arcan_sandbox::ExecRequest`].
///
/// Only environment variables present in `policy.allowed_env` are forwarded
/// to the remote sandbox. The timeout is derived from
/// `policy.max_execution_ms` (converted to whole seconds).
fn to_exec_request(policy: &SandboxPolicy, request: &CommandRequest) -> arcan_sandbox::ExecRequest {
    // Build filtered env map — only allow-listed keys are forwarded.
    let env: HashMap<String, String> = request
        .env
        .iter()
        .filter(|(k, _)| policy.allowed_env.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // argv: [executable, ...args]
    let mut command = vec![request.executable.clone()];
    command.extend(request.args.clone());

    arcan_sandbox::ExecRequest {
        command,
        working_dir: Some(request.cwd.to_string_lossy().into_owned()),
        env,
        timeout_secs: Some(policy.max_execution_ms / 1000),
        stdin: None,
    }
}

/// Translate an [`arcan_sandbox::ExecResult`] into a Praxis [`CommandResult`],
/// applying output truncation from the policy.
fn from_exec_result(
    result: arcan_sandbox::ExecResult,
    policy: &SandboxPolicy,
) -> PraxisResult<CommandResult> {
    let stdout = truncate_utf8(result.stdout, policy.max_stdout_bytes);
    let stderr = truncate_utf8(result.stderr, policy.max_stderr_bytes);
    Ok(CommandResult {
        exit_code: result.exit_code,
        stdout,
        stderr,
    })
}

/// Decode `bytes` as lossy UTF-8, truncating to `max` bytes first.
fn truncate_utf8(bytes: Vec<u8>, max: usize) -> String {
    let mut b = bytes;
    if b.len() > max {
        b.truncate(max);
    }
    String::from_utf8_lossy(&b).into_owned()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use aios_protocol::sandbox::NetworkPolicy;
    use arcan_sandbox::{
        ExecRequest, ExecResult, SandboxCapabilitySet, SandboxError, SandboxHandle, SandboxId,
        SandboxInfo, SandboxSpec, SnapshotId,
    };
    use async_trait::async_trait;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn test_policy() -> SandboxPolicy {
        SandboxPolicy {
            workspace_root: PathBuf::from("/workspace"),
            shell_enabled: true,
            network: NetworkPolicy::Disabled,
            allowed_env: BTreeSet::from(["PATH".to_string(), "HOME".to_string()]),
            max_execution_ms: 5_000,
            max_stdout_bytes: 128,
            max_stderr_bytes: 64,
        }
    }

    fn test_request() -> CommandRequest {
        CommandRequest {
            executable: "/bin/bash".into(),
            args: vec!["-c".into(), "echo hello".into()],
            cwd: PathBuf::from("/workspace"),
            env: vec![
                ("PATH".into(), "/usr/bin".into()),
                ("SECRET".into(), "hunter2".into()),
            ],
        }
    }

    // ── Translation unit tests (no async, no provider) ────────────────────

    #[test]
    fn to_exec_request_builds_correct_argv() {
        let policy = test_policy();
        let request = test_request();
        let exec = to_exec_request(&policy, &request);

        assert_eq!(exec.command[0], "/bin/bash");
        assert_eq!(exec.command[1], "-c");
        assert_eq!(exec.command[2], "echo hello");
        assert_eq!(exec.command.len(), 3);
    }

    #[test]
    fn to_exec_request_filters_env() {
        let policy = test_policy();
        let request = test_request();
        let exec = to_exec_request(&policy, &request);

        // Only PATH is in allowed_env; SECRET must be stripped.
        assert!(exec.env.contains_key("PATH"), "PATH should pass through");
        assert!(!exec.env.contains_key("SECRET"), "SECRET must be filtered");
        assert_eq!(exec.env.len(), 1);
    }

    #[test]
    fn to_exec_request_timeout_from_policy() {
        let policy = test_policy(); // max_execution_ms = 5_000
        let request = test_request();
        let exec = to_exec_request(&policy, &request);

        assert_eq!(exec.timeout_secs, Some(5)); // 5_000 / 1_000
    }

    #[test]
    fn to_exec_request_working_dir_set() {
        let policy = test_policy();
        let request = test_request();
        let exec = to_exec_request(&policy, &request);

        assert_eq!(exec.working_dir.as_deref(), Some("/workspace"));
    }

    #[test]
    fn from_exec_result_truncates_stdout() {
        let policy = test_policy(); // max_stdout_bytes = 128
        let long_stdout = vec![b'x'; 300];
        let result = arcan_sandbox::ExecResult {
            stdout: long_stdout,
            stderr: vec![],
            exit_code: 0,
            duration_ms: 1,
        };
        let cmd = from_exec_result(result, &policy).unwrap();

        assert_eq!(
            cmd.stdout.len(),
            128,
            "stdout must be truncated to 128 bytes"
        );
    }

    #[test]
    fn from_exec_result_maps_exit_code() {
        let policy = test_policy();
        let result = arcan_sandbox::ExecResult {
            stdout: b"ok".to_vec(),
            stderr: vec![],
            exit_code: 42,
            duration_ms: 5,
        };
        let cmd = from_exec_result(result, &policy).unwrap();

        assert_eq!(cmd.exit_code, 42);
        assert_eq!(cmd.stdout, "ok");
    }

    #[test]
    fn from_exec_result_truncates_stderr() {
        let policy = test_policy(); // max_stderr_bytes = 64
        let long_stderr = vec![b'e'; 200];
        let result = arcan_sandbox::ExecResult {
            stdout: vec![],
            stderr: long_stderr,
            exit_code: 1,
            duration_ms: 2,
        };
        let cmd = from_exec_result(result, &policy).unwrap();

        assert_eq!(cmd.stderr.len(), 64, "stderr must be truncated to 64 bytes");
    }

    // ── Mock provider + integration path ─────────────────────────────────────

    struct MockProvider {
        result: ExecResult,
    }

    #[async_trait]
    impl arcan_sandbox::SandboxProvider for MockProvider {
        fn name(&self) -> &'static str {
            "mock"
        }

        fn capabilities(&self) -> SandboxCapabilitySet {
            SandboxCapabilitySet::all()
        }

        async fn create(&self, _spec: SandboxSpec) -> Result<SandboxHandle, SandboxError> {
            unimplemented!("not needed for unit tests")
        }

        async fn resume(&self, _id: &SandboxId) -> Result<SandboxHandle, SandboxError> {
            unimplemented!("not needed for unit tests")
        }

        async fn run(
            &self,
            _id: &SandboxId,
            _req: ExecRequest,
        ) -> Result<ExecResult, SandboxError> {
            Ok(self.result.clone())
        }

        async fn snapshot(&self, _id: &SandboxId) -> Result<SnapshotId, SandboxError> {
            unimplemented!("not needed for unit tests")
        }

        async fn destroy(&self, _id: &SandboxId) -> Result<(), SandboxError> {
            unimplemented!("not needed for unit tests")
        }

        async fn list(&self) -> Result<Vec<SandboxInfo>, SandboxError> {
            unimplemented!("not needed for unit tests")
        }
    }

    #[test]
    fn runner_calls_provider_and_returns_result() {
        let mock_result = ExecResult {
            stdout: b"hello from remote\n".to_vec(),
            stderr: vec![],
            exit_code: 0,
            duration_ms: 10,
        };

        let provider = Arc::new(MockProvider {
            result: mock_result,
        });
        let sandbox_id = SandboxId::from("test-sandbox-1");
        let runner = RemoteCommandRunner::new(provider, sandbox_id);

        let policy = test_policy();
        let request = test_request();

        let result = runner.run(&policy, &request).unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello from remote");
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn runner_rejects_when_shell_disabled() {
        let mock_result = ExecResult {
            stdout: vec![],
            stderr: vec![],
            exit_code: 0,
            duration_ms: 0,
        };
        let provider = Arc::new(MockProvider {
            result: mock_result,
        });
        let sandbox_id = SandboxId::from("test-sandbox-2");
        let runner = RemoteCommandRunner::new(provider, sandbox_id);

        let mut policy = test_policy();
        policy.shell_enabled = false;

        let request = test_request();
        let err = runner.run(&policy, &request).unwrap_err();

        assert!(
            err.to_string().contains("disabled by policy"),
            "expected policy error, got: {err}"
        );
    }
}
