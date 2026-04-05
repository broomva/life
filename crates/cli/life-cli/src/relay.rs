//! `life relay` — manage the relay daemon for remote agent sessions.
//!
//! Wraps life-relayd as a library so users run `life relay auth|start|stop|status`
//! instead of a separate binary. Shares credentials with the broomva CLI
//! (`~/.broomva/config.json`) so `broomva auth login` tokens work here too.

use anyhow::Result;
use clap::Subcommand;
use tracing::info;

#[derive(Subcommand)]
pub enum RelayCommand {
    /// Authenticate with broomva.tech via device authorization.
    /// If already logged in via `broomva auth login`, those credentials are reused.
    Auth {
        /// Server URL.
        #[arg(long, default_value = "https://broomva.tech")]
        url: String,
    },
    /// Start the relay daemon (connects to broomva.tech, polls for commands).
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
    /// Show relay daemon and connection status.
    Status,
}

pub async fn run(command: RelayCommand) -> Result<()> {
    match command {
        RelayCommand::Auth { url } => {
            let cfg = life_relayd::config::load_config()?;

            // Check if broomva CLI token already exists
            match life_relayd::config::read_token(&cfg) {
                Ok(_) => {
                    println!("  Already authenticated (token found).");
                    println!("  Source: broomva CLI or relay credentials.");
                    println!();
                    println!("  To re-authenticate, run `life relay auth --url {url}`");
                    println!("  with a fresh device code flow.");
                }
                Err(_) => {
                    info!(url = %url, "starting device authorization");
                    life_relayd::auth::run(&url, &cfg.credentials_path()).await?;
                }
            }
        }
        RelayCommand::Start { bind, server } => {
            info!(bind = %bind, server = %server, "starting relay daemon");
            life_relayd::daemon::run(&bind, &server).await?;
        }
        RelayCommand::Stop => {
            // Send stop signal to running daemon via local API
            let client = reqwest::Client::new();
            match client
                .get("http://127.0.0.1:3004/health")
                .timeout(std::time::Duration::from_secs(2))
                .send()
                .await
            {
                Ok(_) => {
                    println!("  Relay daemon is running but graceful stop is not yet implemented.");
                    println!("  Use `pkill -f life-relayd` or `kill $(lsof -ti :3004)` to stop.");
                }
                Err(_) => {
                    println!("  No relay daemon running on port 3004.");
                }
            }
        }
        RelayCommand::Status => {
            let cfg = life_relayd::config::load_config()?;
            let has_token = life_relayd::config::read_token(&cfg).is_ok();

            println!("  Relay Configuration");
            println!("  ───────────────────");
            println!("  Config dir:     {}", cfg.config_dir.display());
            println!("  Authenticated:  {}", if has_token { "yes" } else { "no" });

            // Check if daemon is running
            let client = reqwest::Client::new();
            match client
                .get("http://127.0.0.1:3004/health")
                .timeout(std::time::Duration::from_secs(2))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    println!("  Daemon:         running (port 3004)");
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(v) = body.get("version").and_then(|v| v.as_str()) {
                            println!("  Version:        {v}");
                        }
                    }
                }
                _ => {
                    println!("  Daemon:         not running");
                }
            }

            if !has_token {
                println!();
                println!("  Run `life relay auth` or `broomva auth login` to authenticate.");
            }
        }
    }

    Ok(())
}
