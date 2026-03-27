//! Relay daemon configuration.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Relay daemon configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayConfig {
    /// Configuration directory (~/.config/life/relay/).
    #[serde(skip)]
    pub config_dir: PathBuf,
    /// Server URL for WebSocket connection.
    pub server_url: String,
    /// Local API bind address.
    pub bind_address: String,
    /// Node display name.
    pub node_name: String,
}

impl RelayConfig {
    /// Path to the credentials file.
    pub fn credentials_path(&self) -> PathBuf {
        self.config_dir.join("credentials.json")
    }

    /// Path to the local session registry.
    pub fn registry_path(&self) -> PathBuf {
        self.config_dir.join("sessions.json")
    }
}

/// Load configuration from disk, creating defaults if needed.
pub fn load_config() -> Result<RelayConfig> {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("life")
        .join("relay");

    std::fs::create_dir_all(&config_dir)?;

    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_string());

    Ok(RelayConfig {
        config_dir,
        server_url: "wss://broomva.tech/api/relay/connect".to_string(),
        bind_address: "127.0.0.1:3004".to_string(),
        node_name: hostname,
    })
}
