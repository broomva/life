//! `soma` — Life Agent OS kernel daemon binary.
//!
//! Single binary that hosts the privileged µVM hypervisor (`soma daemon`)
//! and the operator CLI (`soma create-vm`, `soma dispatch`, `soma list-vms`).
//!
//! Pairs with `anima` (identity / soul). One binary per substrate,
//! per Spec C §L1.

#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use soma::SomaConfig;

#[derive(Debug, Parser)]
#[command(
    name = "soma",
    version,
    about = "Life Agent OS kernel daemon — body of the agent",
    long_about = "Privileged daemon implementing the aiOS kernel contract for the µVM isolation tier.\n\
                  Hosts a tonic KernelService on a Unix socket. The same binary serves as the operator CLI\n\
                  for inspecting VMs, dispatching to running VMs, and listing live state."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the kernel daemon (default mode for systemd unit `soma.service`).
    Daemon {
        /// Path to config.toml.
        ///
        /// Defaults to the built-in defaults (equivalent to an empty file)
        /// when absent. The `SOMA_CONFIG` environment variable is also
        /// accepted.
        #[arg(long, env = "SOMA_CONFIG")]
        config: Option<PathBuf>,
    },

    /// Create a new VM via the kernel daemon.
    CreateVm(CliFlags<soma::cli::create_vm::Args>),

    /// Dispatch a request to a running VM.
    Dispatch(CliFlags<soma::cli::dispatch::Args>),

    /// List VMs known to the daemon (optionally scoped to a session).
    ListVms(CliFlags<soma::cli::list_vms::Args>),
}

#[derive(Debug, Parser)]
struct CliFlags<A: clap::Args> {
    /// Path to the soma Unix socket.
    #[arg(long, env = "SOMA_SOCKET", default_value = "/run/life/soma.sock")]
    socket: PathBuf,

    #[command(flatten)]
    args: A,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Daemon { config } => run_daemon(config).await,
        Cmd::CreateVm(f) => soma::cli::create_vm::run(&f.socket, f.args).await,
        Cmd::Dispatch(f) => soma::cli::dispatch::run(&f.socket, f.args).await,
        Cmd::ListVms(f) => soma::cli::list_vms::run(&f.socket, f.args).await,
    }
}

async fn run_daemon(config: Option<PathBuf>) -> anyhow::Result<()> {
    let cfg = SomaConfig::load(config.as_deref())?;

    // Initialise Vigil telemetry (tracing subscriber + optional OTLP export).
    // The guard must live for the full process lifetime — when dropped it
    // flushes pending spans and metrics to the collector.
    let _vigil_guard = soma::observability::init(&cfg.vigil)?;

    tracing::info!(
        socket = %cfg.server.unix_socket.display(),
        namespace = %cfg.lago.namespace,
        "soma daemon starting",
    );

    let bootstrap = soma::bootstrap::build_engine(&cfg).await?;
    tracing::info!(
        vms_replayed = bootstrap.replayed.live_vms.len(),
        events_applied = bootstrap.replayed.events_applied,
        "replay complete",
    );

    let seed = bootstrap.replayed.snapshot_vm_handles();
    let shutdown_rx = soma::shutdown::install_signal_handler();

    // Hold `_tempdir` until the end of `main` so the in-memory journal's
    // backing file is not deleted while the daemon is running.
    let _tempdir = bootstrap._lago_tempdir;

    // Spec D D-Sub-E: spawn the admin custody-oracle UDS in parallel
    // with the kernel UDS. Disabled when no `[admin_plane]` section is
    // configured. The handle is held until the kernel listener
    // returns; on shutdown we abort it so the kernel UDS drains
    // unblocked.
    let admin_handle = if cfg.admin_plane.is_some() {
        // Operators provision keys via a future ticket / management
        // RPC. For now the admin plane starts with an empty key store
        // — calls for unprovisioned users return NotFound. This keeps
        // the wire surface live for SomaCustody integration tests
        // (which provision their own keys via the test harness) while
        // making accidental production exposure fail-closed.
        Some(soma::admin::run_admin_plane(&cfg).await?)
    } else {
        None
    };

    let serve_result = soma::listener::serve(&cfg, bootstrap.engine, shutdown_rx, seed).await;

    if let Some(handle) = admin_handle {
        handle.abort();
    }

    serve_result?;

    tracing::info!("soma daemon stopped");
    Ok(())
}
