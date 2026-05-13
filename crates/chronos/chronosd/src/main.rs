//! chronosd — Chronos daemon binary.
//!
//! Thin CLI front-end. All runtime logic lives in [`chronosd`] the library; see
//! `src/lib.rs`.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use chronosd::{DaemonConfig, run};

/// CLI surface for `chronosd`. See `chronosd --help` for the rendered version.
#[derive(Debug, Parser)]
#[command(
    name = "chronosd",
    about = "Chronos daemon — the temporal substrate of the Life Agent OS",
    version
)]
struct Cli {
    /// Heartbeat tick interval in seconds. Default 10. Use 60+ in production to keep
    /// the system-session journal quiet.
    #[arg(long, default_value_t = 10, env = "CHRONOSD_HEARTBEAT_SECONDS")]
    heartbeat_seconds: u64,

    /// Directory holding the lago redb journal. Created on startup if missing.
    #[arg(long, default_value = "/tmp/chronosd", env = "CHRONOSD_DATA_DIR")]
    data_dir: PathBuf,

    /// File name (within `--data-dir`) for the redb journal.
    #[arg(long, default_value = "journal.redb", env = "CHRONOSD_JOURNAL_FILENAME")]
    journal_filename: String,

    /// Mpsc buffer capacity for the [`chronos_core::WakeRouter`]. Larger values absorb
    /// trigger bursts; smaller values surface backpressure faster.
    #[arg(long, default_value_t = 64, env = "CHRONOSD_ROUTER_BUFFER")]
    router_buffer: usize,
}

impl From<Cli> for DaemonConfig {
    fn from(cli: Cli) -> Self {
        DaemonConfig {
            heartbeat_interval: Duration::from_secs(cli.heartbeat_seconds.max(1)),
            data_dir: cli.data_dir,
            journal_filename: cli.journal_filename,
            router_buffer: cli.router_buffer.max(1),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Standard tracing setup: env-filter for selective verbosity, compact event formatting.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    let cli = Cli::parse();
    let config: DaemonConfig = cli.into();
    run(config, None).await
}
