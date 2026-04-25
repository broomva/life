//! `lifectl list-vms` — list VMs managed by the lifed daemon.

use std::path::Path;

use anyhow::{Context, Result};
use life_kernel_proto::pb;
use tokio_stream::StreamExt;

/// Arguments for the `list-vms` subcommand.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Restrict listing to a single session (optional).
    #[arg(long)]
    pub session: Option<String>,

    /// Emit JSON on stdout instead of a human-readable table.
    #[arg(long)]
    pub json: bool,
}

/// Run `list-vms` against the daemon at `socket`.
pub async fn run(socket: &Path, args: Args) -> Result<()> {
    let mut client = crate::client::connect(socket).await?;
    let request = build_request(&args);

    let mut stream = client
        .list_vms(request)
        .await
        .context("list_vms RPC failed")?
        .into_inner();

    let mut entries: Vec<pb::VmInfo> = Vec::new();
    while let Some(item) = stream.next().await {
        entries.push(item.context("stream item error")?);
    }

    if args.json {
        let json: Vec<_> = entries
            .iter()
            .map(|info| {
                serde_json::json!({
                    "vm_id": info.vm_id.as_ref().map(|v| v.value.as_str()),
                    "backend": info.backend.as_ref().map(|b| b.value.as_str()),
                    "status": info.status.as_ref().map(|s| s.state.as_str()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("{:<36}  {:<12}  STATUS", "VM_ID", "BACKEND");
        for info in entries {
            let vm_id = info.vm_id.as_ref().map(|v| v.value.as_str()).unwrap_or("");
            let backend = info
                .backend
                .as_ref()
                .map(|b| b.value.as_str())
                .unwrap_or("");
            let status = info.status.as_ref().map(|s| s.state.as_str()).unwrap_or("");
            println!("{vm_id:<36}  {backend:<12}  {status}");
        }
    }
    Ok(())
}

/// Construct the tonic request from CLI args.
///
/// Exported as `pub(crate)` so tests can exercise the request-building logic
/// without a live daemon connection.
pub(crate) fn build_request(args: &Args) -> tonic::Request<pb::ListVmsRequest> {
    tonic::Request::new(pb::ListVmsRequest {
        session_id: args.session.as_deref().map(|s| pb::SessionId {
            value: s.to_owned(),
        }),
    })
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_no_session_filter() {
        let args = Args {
            session: None,
            json: false,
        };
        let req = build_request(&args);
        let inner = req.into_inner();
        assert!(inner.session_id.is_none());
    }

    #[test]
    fn build_request_with_session_filter() {
        let args = Args {
            session: Some("my-session".to_owned()),
            json: false,
        };
        let req = build_request(&args);
        let inner = req.into_inner();
        let sid = inner.session_id.unwrap();
        assert_eq!(sid.value, "my-session");
    }
}
