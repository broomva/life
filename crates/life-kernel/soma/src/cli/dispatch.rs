//! `soma dispatch` — dispatch a tool call into an existing VM.

use std::path::Path;

use anyhow::{Context, Result, bail};
use life_kernel_proto::pb;

/// Arguments for the `dispatch` subcommand.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// VM ID (returned by `create-vm`).
    #[arg(long)]
    pub vm_id: String,

    /// Backend that owns the VM (must match the VM's handle).
    #[arg(long, default_value = "local")]
    pub backend: String,

    /// Tool name to invoke.
    #[arg(long)]
    pub tool_name: String,

    /// Tool input as a JSON string.
    #[arg(long, default_value = "{}")]
    pub input: String,

    /// Session ID for the dispatch context.
    #[arg(long, default_value = "soma")]
    pub session: String,

    /// Agent ID for the dispatch context.
    #[arg(long, default_value = "soma")]
    pub agent: String,

    /// Emit JSON on stdout instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

/// Run `dispatch` against the daemon at `socket`.
pub async fn run(socket: &Path, args: Args) -> Result<()> {
    let mut client = crate::cli::client::connect(socket).await?;
    let request = build_request(&args)?;
    let response = client
        .dispatch(request)
        .await
        .context("dispatch RPC failed")?
        .into_inner();

    if args.json {
        let json = serde_json::json!({
            "call_id": response.call_id,
            "tool_name": response.tool_name,
            "is_error": response.is_error,
            "output_json": String::from_utf8_lossy(&response.output_json),
            "usage": response.usage.as_ref().map(|u| serde_json::json!({
                "cpu_ms": u.cpu_ms,
                "duration_ms": u.duration_ms,
                "egress_bytes": u.egress_bytes,
                "syscall_count": u.syscall_count,
                "confidence": u.confidence,
            })),
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        let output = String::from_utf8_lossy(&response.output_json);
        println!(
            "call_id={} tool={} is_error={}",
            response.call_id, response.tool_name, response.is_error
        );
        if let Some(usage) = response.usage.as_ref() {
            println!(
                "usage: cpu_ms={} duration_ms={} egress_bytes={}",
                usage.cpu_ms, usage.duration_ms, usage.egress_bytes,
            );
        }
        println!("output: {output}");
    }

    if response.is_error {
        bail!("dispatch returned is_error=true");
    }
    Ok(())
}

/// Construct the tonic request from CLI args.
///
/// Exported as `pub(crate)` so tests can exercise the request-building logic
/// without a live daemon connection.
pub(crate) fn build_request(args: &Args) -> Result<tonic::Request<pb::DispatchRequest>> {
    let input_bytes = args.input.as_bytes().to_vec();

    // Build a stub VmHandle from the caller-supplied IDs. The daemon will
    // resolve the real handle from its live-VM registry.
    let vm = pb::VmHandle {
        vm_id: Some(pb::VmId {
            value: args.vm_id.clone(),
        }),
        backend: Some(pb::BackendId {
            value: args.backend.clone(),
        }),
        session_id: Some(pb::SessionId {
            value: args.session.clone(),
        }),
        agent_id: Some(pb::AgentId {
            value: args.agent.clone(),
        }),
        status: Some(pb::VmStatus {
            state: "running".to_owned(),
            reason: String::new(),
        }),
        created_at: None,
        metadata_json: vec![],
    };

    let call = pb::ToolCall {
        call_id: uuid_v4_hex(),
        tool_name: args.tool_name.clone(),
        input_json: input_bytes,
        requested_capabilities: vec![],
    };

    let ctx = pb::KernelContext {
        session_id: Some(pb::SessionId {
            value: args.session.clone(),
        }),
        agent_id: Some(pb::AgentId {
            value: args.agent.clone(),
        }),
        wallet: Some(pb::WalletAttribution {
            address: "0x0000000000000000000000000000000000000000".to_owned(),
            chain_caip2: "eip155:8453".to_owned(),
        }),
        cost_hint: None,
        trace_ctx: None,
    };

    Ok(tonic::Request::new(pb::DispatchRequest {
        vm: Some(vm),
        call: Some(call),
        ctx: Some(ctx),
    }))
}

/// Generate a short pseudo-unique hex string for call IDs in the CLI context.
///
/// Not cryptographically random — good enough for operator debug invocations
/// where uniqueness within a session matters but security does not.
fn uuid_v4_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("soma-{nanos:08x}")
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_args() -> Args {
        Args {
            vm_id: "vm-abc123".to_owned(),
            backend: "local".to_owned(),
            tool_name: "bash".to_owned(),
            input: r#"{"command": "echo hello"}"#.to_owned(),
            session: "sess-1".to_owned(),
            agent: "agent-1".to_owned(),
            json: false,
        }
    }

    #[test]
    fn build_request_sets_vm_id() {
        let args = default_args();
        let req = build_request(&args).unwrap();
        let inner = req.into_inner();
        let vm = inner.vm.unwrap();
        assert_eq!(vm.vm_id.unwrap().value, "vm-abc123");
        assert_eq!(vm.backend.unwrap().value, "local");
    }

    #[test]
    fn build_request_sets_tool_name() {
        let args = default_args();
        let req = build_request(&args).unwrap();
        let inner = req.into_inner();
        let call = inner.call.unwrap();
        assert_eq!(call.tool_name, "bash");
    }

    #[test]
    fn build_request_encodes_input_json() {
        let args = default_args();
        let req = build_request(&args).unwrap();
        let inner = req.into_inner();
        let call = inner.call.unwrap();
        let decoded = String::from_utf8(call.input_json).unwrap();
        assert_eq!(decoded, r#"{"command": "echo hello"}"#);
    }

    #[test]
    fn build_request_sets_context() {
        let args = default_args();
        let req = build_request(&args).unwrap();
        let inner = req.into_inner();
        let ctx = inner.ctx.unwrap();
        assert_eq!(ctx.session_id.unwrap().value, "sess-1");
        assert_eq!(ctx.agent_id.unwrap().value, "agent-1");
    }
}
