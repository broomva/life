//! `soma create-vm` — provision a new VM in the soma daemon.

use std::path::Path;

use anyhow::{Context, Result};
use life_kernel_proto::{aios_v1, pb};

/// Arguments for the `create-vm` subcommand.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Backend selector: `local`, `cube`, `vercel`, or `auto`.
    #[arg(long, default_value = "auto")]
    pub backend: String,

    /// Number of vCPUs.
    #[arg(long, default_value_t = 1)]
    pub vcpus: u32,

    /// Memory in KiB.
    #[arg(long, default_value_t = 262_144)]
    pub memory_kb: u64,

    /// Runtime hint: `shell`, `node`, or `python`.
    #[arg(long, default_value = "shell")]
    pub runtime: String,

    /// Session ID to associate with the created VM.
    #[arg(long, default_value = "soma")]
    pub session: String,

    /// Agent ID to associate with the created VM.
    #[arg(long, default_value = "soma")]
    pub agent: String,

    /// Wallet address for attribution (required by KernelContext).
    #[arg(long, default_value = "0x0000000000000000000000000000000000000000")]
    pub wallet: String,

    /// Wallet chain in CAIP-2 form (e.g. `eip155:8453`).
    #[arg(long, default_value = "eip155:8453")]
    pub wallet_chain: String,

    /// Emit JSON on stdout instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

/// Run `create-vm` against the daemon at `socket`.
pub async fn run(socket: &Path, args: Args) -> Result<()> {
    let mut client = crate::cli::client::connect(socket).await?;
    let request = build_request(&args);
    let response = client
        .create_vm(request)
        .await
        .context("create_vm RPC failed")?
        .into_inner();

    if args.json {
        // pb::VmHandle is not Serialize; construct a JSON value manually.
        let json = serde_json::json!({
            "vm_id": response.vm_id.as_ref().map(|v| v.value.as_str()),
            "backend": response.backend.as_ref().map(|b| b.value.as_str()),
            "session_id": response.session_id.as_ref().map(|s| s.value.as_str()),
            "agent_id": response.agent_id.as_ref().map(|a| a.value.as_str()),
            "status": response.status.as_ref().map(|s| s.state.as_str()),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).context("serializing handle")?
        );
    } else {
        let vm_id = response
            .vm_id
            .as_ref()
            .map(|v| v.value.as_str())
            .unwrap_or("");
        let backend = response
            .backend
            .as_ref()
            .map(|b| b.value.as_str())
            .unwrap_or("");
        let session = response
            .session_id
            .as_ref()
            .map(|s| s.value.as_str())
            .unwrap_or("");
        println!("vm_id={vm_id} backend={backend} session={session}");
    }
    Ok(())
}

/// Construct the tonic request from CLI args.
///
/// Exported as `pub(crate)` so tests can exercise the request-building logic
/// without spinning up a real daemon socket.
pub(crate) fn build_request(args: &Args) -> tonic::Request<pb::CreateVmRequest> {
    let backend_selector = match args.backend.as_str() {
        "auto" => pb::BackendSelector {
            kind: Some(pb::backend_selector::Kind::Auto(pb::Empty {})),
        },
        explicit => pb::BackendSelector {
            kind: Some(pb::backend_selector::Kind::Explicit(aios_v1::BackendId {
                value: explicit.to_owned(),
            })),
        },
    };

    let runtime_hint_kind = match args.runtime.as_str() {
        "node" => pb::RuntimeHintKind::RuntimeHintNode as i32,
        "python" => pb::RuntimeHintKind::RuntimeHintPython as i32,
        _ => pb::RuntimeHintKind::RuntimeHintShell as i32,
    };

    let spec = pb::VmSpec {
        backend_selector: Some(backend_selector),
        resources: Some(pb::VmResources {
            vcpus: args.vcpus,
            memory_kb: args.memory_kb,
            disk_kb: 0,
            timeout_secs: 0,
        }),
        network_policy_json: b"null".to_vec(),
        mounts: vec![],
        env: std::collections::HashMap::new(),
        runtime_hint: Some(pb::RuntimeHint {
            kind: runtime_hint_kind,
            version_or_image: String::new(),
        }),
        labels: std::collections::HashMap::new(),
    };

    let ctx = pb::KernelContext {
        session_id: Some(aios_v1::SessionId {
            value: args.session.clone(),
        }),
        agent_id: Some(aios_v1::AgentId {
            value: args.agent.clone(),
        }),
        wallet: Some(pb::WalletAttribution {
            address: args.wallet.clone(),
            chain_caip2: args.wallet_chain.clone(),
        }),
        cost_hint: None,
        trace_ctx: None,
    };

    tonic::Request::new(pb::CreateVmRequest {
        spec: Some(spec),
        ctx: Some(ctx),
    })
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_args() -> Args {
        Args {
            backend: "auto".to_owned(),
            vcpus: 2,
            memory_kb: 524_288,
            runtime: "shell".to_owned(),
            session: "test-sess".to_owned(),
            agent: "test-agent".to_owned(),
            wallet: "0xdead".to_owned(),
            wallet_chain: "eip155:8453".to_owned(),
            json: false,
        }
    }

    #[test]
    fn build_request_auto_backend() {
        let args = default_args();
        let req = build_request(&args);
        let inner = req.into_inner();
        let spec = inner.spec.unwrap();
        let sel = spec.backend_selector.unwrap();
        assert!(matches!(
            sel.kind,
            Some(pb::backend_selector::Kind::Auto(_))
        ));
        assert_eq!(spec.resources.unwrap().vcpus, 2);
        assert_eq!(spec.resources.unwrap().memory_kb, 524_288);
    }

    #[test]
    fn build_request_explicit_backend() {
        let mut args = default_args();
        args.backend = "local".to_owned();
        let req = build_request(&args);
        let inner = req.into_inner();
        let spec = inner.spec.unwrap();
        let sel = spec.backend_selector.unwrap();
        match sel.kind {
            Some(pb::backend_selector::Kind::Explicit(id)) => {
                assert_eq!(id.value, "local");
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
    }

    #[test]
    fn build_request_runtime_hints() {
        for (runtime, expected_kind) in [
            ("shell", pb::RuntimeHintKind::RuntimeHintShell as i32),
            ("node", pb::RuntimeHintKind::RuntimeHintNode as i32),
            ("python", pb::RuntimeHintKind::RuntimeHintPython as i32),
            ("unknown", pb::RuntimeHintKind::RuntimeHintShell as i32),
        ] {
            let mut args = default_args();
            args.runtime = runtime.to_owned();
            let req = build_request(&args);
            let inner = req.into_inner();
            let hint = inner.spec.unwrap().runtime_hint.unwrap();
            assert_eq!(hint.kind, expected_kind, "runtime={runtime}");
        }
    }

    #[test]
    fn build_request_kernel_context() {
        let args = default_args();
        let req = build_request(&args);
        let inner = req.into_inner();
        let ctx = inner.ctx.unwrap();
        assert_eq!(ctx.session_id.unwrap().value, "test-sess");
        assert_eq!(ctx.agent_id.unwrap().value, "test-agent");
        let wallet = ctx.wallet.unwrap();
        assert_eq!(wallet.address, "0xdead");
        assert_eq!(wallet.chain_caip2, "eip155:8453");
    }
}
