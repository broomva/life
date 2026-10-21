//! `cuenca-server`: run the Cuenca authority. The `cuenca` CLI's `serve` (BRO-3252)
//! runs the same [`cuenca_server::run`].

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

/// Run the Cuenca authority: the agent API on the tailnet, owner acts on a local socket.
#[derive(Parser)]
#[command(name = "cuenca-server", version)]
struct Args {
    /// State directory: the log, blobs and keys file.
    #[arg(long, env = "CUENCA_STATE")]
    state: PathBuf,
    /// Address for the agent API: this host's tailnet address and a port.
    #[arg(long, env = "CUENCA_BIND")]
    bind: SocketAddr,
    /// Path of the admin socket.
    #[arg(long, env = "CUENCA_ADMIN_SOCKET")]
    admin_socket: PathBuf,
    /// Bind an address outside the tailnet ranges.
    #[arg(long)]
    unsafe_bind: bool,
    /// The tailscale binary.
    #[arg(long, default_value = "tailscale")]
    tailscale: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let a = Args::parse();
    cuenca_server::run(cuenca_server::Config {
        state: a.state,
        bind: a.bind,
        admin_socket: a.admin_socket,
        unsafe_bind: a.unsafe_bind,
        tailscale: a.tailscale,
    })
    .await
}
