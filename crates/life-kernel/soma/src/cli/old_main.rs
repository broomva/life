//! `lifectl` — operator CLI for the soma kernel daemon.

#![deny(unsafe_code)]

mod client;
mod commands;

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "lifectl",
    version,
    about = "Operator CLI for the lifed kernel daemon"
)]
struct Cli {
    /// Path to the soma Unix socket.
    #[arg(long, env = "SOMA_SOCKET", default_value = "/run/life/soma.sock")]
    socket: PathBuf,

    #[command(subcommand)]
    cmd: commands::Command,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    cli.cmd.run(&cli.socket).await
}
