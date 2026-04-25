//! `soma` — daemon entrypoint.
//!
//! Parses CLI flags, loads configuration, replays the event journal to
//! reconstruct live-VM state, installs signal handlers, and then runs the
//! tonic server until SIGINT / SIGTERM fires and all in-flight dispatches
//! drain.

#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::Parser;
use soma::SomaConfig;

#[derive(Debug, Parser)]
#[command(
    name = "lifed",
    version,
    about = "Life Agent OS kernel daemon",
    long_about = "Privileged daemon implementing the aiOS kernel contract for the µVM isolation tier.\n\
                  Listens on a Unix socket and exposes the KernelService gRPC API."
)]
struct Cli {
    /// Path to config.toml.
    ///
    /// Defaults to the built-in defaults (equivalent to an empty file) when
    /// absent.  The `SOMA_CONFIG` environment variable is also accepted.
    #[arg(long, env = "SOMA_CONFIG")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = SomaConfig::load(cli.config.as_deref())?;

    // Initialise Vigil telemetry (tracing subscriber + optional OTLP export).
    // The guard must live for the full process lifetime — when dropped it
    // flushes pending spans and metrics to the collector.
    let _vigil_guard = soma::observability::init(&cfg.vigil)?;

    tracing::info!(
        socket = %cfg.server.unix_socket.display(),
        namespace = %cfg.lago.namespace,
        "lifed starting",
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

    soma::listener::serve(&cfg, bootstrap.engine, shutdown_rx, seed).await?;

    tracing::info!("lifed stopped");
    Ok(())
}
