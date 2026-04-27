//! `lifegw` — Life Runtime edge gateway daemon.

#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "lifegw",
    version,
    about = "Life Runtime edge gateway daemon — TLS termination + Tier-2 mint + tonic-web proxy",
    long_about = "Terminates TLS, verifies Tier-1 identity JWTs, mints Tier-2 capability\n\
                  tokens, and forwards life.v1.* RPCs to lifed via /run/life/life.sock.\n\
                  Real ES256 + Vercel JWKS lands in Sub-phase B; WS in C; rate-limit in D."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the daemon (default mode for systemd unit `lifegw.service`).
    Daemon {
        #[arg(long, env = "LIFEGW_CONFIG")]
        config: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Daemon { config } => {
            lifegw::bootstrap::run_daemon(config.as_deref()).await?;
            Ok(())
        }
    }
}
