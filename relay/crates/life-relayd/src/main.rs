//! life-relayd — relay daemon for remote agent sessions.
//!
//! Connects to broomva.tech via outbound HTTP polling, bridges local
//! agent sessions (Claude Code, Codex, Arcan) to the web UI.

use life_relayd::{auth, config, daemon};

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};

#[derive(Parser)]
#[command(
    name = "relayd",
    version,
    about = "Life Relay daemon — remote agent sessions"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate with broomva.tech via device authorization.
    Auth {
        /// Server URL (default: <https://broomva.tech>).
        #[arg(long, default_value = "https://broomva.tech")]
        url: String,
    },
    /// Start the relay daemon.
    Start {
        /// Local API bind address.
        #[arg(long, default_value = "127.0.0.1:3004")]
        bind: String,
        /// Server URL to connect to.
        #[arg(long, default_value = "https://broomva.tech")]
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
            let cfg = config::load_config()?;
            auth::run(&url, &cfg.credentials_path()).await?;
        }
        Command::Start { bind, server } => {
            info!(bind = %bind, server = %server, "starting life-relayd");
            daemon::run(&bind, &server).await?;
        }
        Command::Stop => {
            warn!("stop signal not yet implemented");
        }
        Command::Status => {
            let cfg = config::load_config()?;
            info!(config_dir = %cfg.config_dir.display(), authenticated = cfg.credentials_path().exists(), "relay status");
        }
    }

    Ok(())
}
