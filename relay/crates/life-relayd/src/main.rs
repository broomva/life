//! life-relayd — relay daemon for remote agent sessions.
//!
//! Connects to broomva.tech via outbound WebSocket, bridges local
//! agent sessions (Claude Code, Codex, Arcan) to the web UI.

mod config;
mod connection;
mod daemon;
mod adapters;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;

#[derive(Parser)]
#[command(name = "relayd", version, about = "Life Relay daemon — remote agent sessions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate with broomva.tech via device authorization.
    Auth {
        /// Server URL (default: https://broomva.tech).
        #[arg(long, default_value = "https://broomva.tech")]
        url: String,
    },
    /// Start the relay daemon.
    Start {
        /// Local API bind address.
        #[arg(long, default_value = "127.0.0.1:3004")]
        bind: String,
        /// Server URL to connect to.
        #[arg(long, default_value = "wss://broomva.tech/api/relay/connect")]
        server: String,
    },
    /// Stop the relay daemon.
    Stop,
    /// Show daemon and connection status.
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "life_relayd=info,life_relay=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Auth { url } => {
            info!(url = %url, "starting device authorization");
            eprintln!("Device authorization not yet implemented.");
            eprintln!("Will open browser to {url}/device?code=XXXX");
        }
        Command::Start { bind, server } => {
            info!(bind = %bind, server = %server, "starting life-relayd");
            daemon::run(&bind, &server).await?;
        }
        Command::Stop => {
            eprintln!("Sending stop signal to running daemon...");
            // TODO: signal via PID file or local API
        }
        Command::Status => {
            let cfg = config::load_config()?;
            eprintln!("Config dir: {}", cfg.config_dir.display());
            eprintln!("Authenticated: {}", cfg.credentials_path().exists());
        }
    }

    Ok(())
}
