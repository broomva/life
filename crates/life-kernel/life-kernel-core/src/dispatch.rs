//! Pure translators between
//! [`aios_protocol::tool::ToolCall`]/[`aios_protocol::tool::ToolResult`]
//! and [`aios_protocol::hypervisor::ExecRequest`]/
//! [`aios_protocol::hypervisor::ExecResult`].
//!
//! These are Phase 1 translators: Tool-ABI dispatch into shell-level
//! invocations is deliberately minimal so the engine wiring (gate chain
//! → registry → metering → backend) can be validated end-to-end without
//! pulling in the full Tool → argv translation machinery. The conformance
//! suite (BRO-872) drives the resulting shell against
//! `arcan-provider-local`, so the translator emits a `/bin/sh -c echo`
//! trace that the suite can assert on.
//!
//! ### Out of scope
//!
//! Real Tool input-schema → argv translation (JSON → POSIX argv with
//! quoting, per-tool mapping, MCP semantics) is owned by the Tool
//! implementations behind the [`HypervisorBackend`][aios_protocol::hypervisor::HypervisorBackend]
//! itself. This translator only proves that the engine plumbs calls end
//! to end.

use std::collections::HashMap;

use aios_protocol::budget::ResourceUsage;
use aios_protocol::hypervisor::{ExecRequest, ExecResult, RuntimeHint};
use aios_protocol::tool::{ToolCall, ToolContent, ToolResult};

/// Phase 1 default timeout applied to translated [`ExecRequest`]s.
///
/// Exposed as a constant so the conformance suite can cross-check the
/// wire form without duplicating the magic number.
pub const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 30;

/// Translate a [`ToolCall`] into an [`ExecRequest`] for the hypervisor
/// backend.
///
/// Phase 1 uses a single shell-backed invocation for every call regardless
/// of `runtime`: `/bin/sh -c "echo '{call_id}:{tool_name}'"`. The call id
/// and tool name are echoed back so the conformance suite can assert that
/// the engine forwarded the request unmodified. Node / Python / Custom
/// runtimes are accepted but do not change the shape in Phase 1 — the real
/// per-runtime dispatch arrives with the Tool-ABI work in Phase 3.
///
/// The request carries an empty env map and the default
/// [`DEFAULT_EXEC_TIMEOUT_SECS`] timeout so the backend does not wait
/// indefinitely for a misbehaving echo. Stdin and working dir are left
/// unset — Phase 1 tools have no input channel yet.
pub fn tool_call_to_exec_request(call: &ToolCall, _runtime: &RuntimeHint) -> ExecRequest {
    ExecRequest {
        command: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("echo '{}:{}'", call.call_id, call.tool_name),
        ],
        working_dir: None,
        env: HashMap::new(),
        timeout_secs: Some(DEFAULT_EXEC_TIMEOUT_SECS),
        stdin: None,
    }
}

/// Translate an [`ExecResult`] (plus kernel-tier [`ResourceUsage`]) into
/// a [`ToolResult`].
///
/// Populates:
///
/// - `call_id` / `tool_name`: echoed from the original call so the
///   kernel dispatch path stays correlatable.
/// - `output`: a JSON object `{ "stdout", "stderr", "exit_code" }`
///   preserving everything the backend reported so callers can inspect
///   the raw exec output without losing information.
/// - `content`: one [`ToolContent::Text`] block holding the UTF-8
///   lossy decode of stdout — the legacy shape consumed by today's
///   Arcan tool harness.
/// - `is_error`: `true` iff `exec.exit_code != 0`, matching the POSIX
///   convention.
/// - `usage`: threaded through for downstream billing / audit.
pub fn exec_result_to_tool_result(
    result: ExecResult,
    usage: ResourceUsage,
    call_id: String,
    tool_name: String,
) -> ToolResult {
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    ToolResult {
        call_id,
        tool_name,
        output: serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": result.exit_code,
        }),
        content: Some(vec![ToolContent::Text { text: stdout }]),
        is_error: result.exit_code != 0,
        usage: Some(usage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aios_protocol::budget::UsageConfidence;

    fn canned_call(call_id: &str, tool_name: &str) -> ToolCall {
        ToolCall {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            input: serde_json::json!({ "foo": "bar" }),
            requested_capabilities: Vec::new(),
        }
    }

    fn canned_usage() -> ResourceUsage {
        ResourceUsage {
            cpu_ms: 12,
            mem_peak_kb: 2_048,
            egress_bytes: 0,
            duration_ms: 50,
            syscall_count: 7,
            confidence: UsageConfidence::Estimated,
        }
    }

    #[test]
    fn tool_call_to_exec_request_shapes_shell_echo() {
        let call = canned_call("call-1", "tool.greet");
        let req = tool_call_to_exec_request(&call, &RuntimeHint::Shell);

        assert_eq!(req.command.len(), 3);
        assert_eq!(req.command[0], "/bin/sh");
        assert_eq!(req.command[1], "-c");
        assert_eq!(req.command[2], "echo 'call-1:tool.greet'");
        assert_eq!(req.timeout_secs, Some(DEFAULT_EXEC_TIMEOUT_SECS));
        assert!(req.stdin.is_none());
        assert!(req.working_dir.is_none());
        assert!(req.env.is_empty());
    }

    #[test]
    fn tool_call_to_exec_request_ignores_runtime_hint_variant() {
        // Phase 1 contract: runtime_hint does not alter the shape.
        let call = canned_call("call-2", "tool.any");
        let req_shell = tool_call_to_exec_request(&call, &RuntimeHint::Shell);
        let req_node = tool_call_to_exec_request(
            &call,
            &RuntimeHint::Node {
                version: "20".into(),
            },
        );
        let req_python = tool_call_to_exec_request(
            &call,
            &RuntimeHint::Python {
                version: "3.12".into(),
            },
        );
        let req_custom = tool_call_to_exec_request(
            &call,
            &RuntimeHint::Custom {
                image: "ghcr.io/foo/bar:latest".into(),
            },
        );
        assert_eq!(req_shell.command, req_node.command);
        assert_eq!(req_shell.command, req_python.command);
        assert_eq!(req_shell.command, req_custom.command);
    }

    #[test]
    fn exec_result_to_tool_result_preserves_usage() {
        let exec = ExecResult {
            stdout: b"hello world\n".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
            duration_ms: 42,
        };
        let usage = canned_usage();
        let result =
            exec_result_to_tool_result(exec, usage.clone(), "call-1".into(), "tool.greet".into());

        assert_eq!(result.call_id, "call-1");
        assert_eq!(result.tool_name, "tool.greet");
        assert!(!result.is_error);
        assert_eq!(result.output["stdout"], "hello world\n");
        assert_eq!(result.output["stderr"], "");
        assert_eq!(result.output["exit_code"], 0);
        let usage_out = result.usage.as_ref().expect("usage threaded through");
        assert_eq!(usage_out, &usage);
        match result.content.as_deref() {
            Some([ToolContent::Text { text }]) => assert_eq!(text, "hello world\n"),
            other => panic!("expected single Text content block, got {other:?}"),
        }
    }

    #[test]
    fn exec_result_to_tool_result_marks_is_error_on_nonzero_exit() {
        let exec = ExecResult {
            stdout: b"boom".to_vec(),
            stderr: b"permission denied".to_vec(),
            exit_code: 127,
            duration_ms: 1,
        };
        let result =
            exec_result_to_tool_result(exec, canned_usage(), "call-err".into(), "tool.fail".into());

        assert!(result.is_error, "non-zero exit must set is_error");
        assert_eq!(result.output["exit_code"], 127);
        assert_eq!(result.output["stderr"], "permission denied");
    }
}
