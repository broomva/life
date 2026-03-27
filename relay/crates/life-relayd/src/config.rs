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
    /// Base server URL (e.g. <https://broomva.tech>).
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
    #[allow(dead_code)]
    pub fn registry_path(&self) -> PathBuf {
        self.config_dir.join("sessions.json")
    }
}

/// Read stored auth token from broomva CLI config or relay credentials.
pub fn read_token(cfg: &RelayConfig) -> Result<String> {
    // First try relay-specific credentials
    let relay_creds = cfg.credentials_path();
    if relay_creds.exists() {
        let content = std::fs::read_to_string(&relay_creds)?;
        let parsed: serde_json::Value = serde_json::from_str(&content)?;
        if let Some(token) = parsed.get("token").and_then(|t| t.as_str()) {
            return Ok(token.to_string());
        }
    }

    // Fall back to broomva CLI config (~/.config/broomva/config.json)
    let broomva_config = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("broomva")
        .join("config.json");

    if broomva_config.exists() {
        let content = std::fs::read_to_string(&broomva_config)?;
        let parsed: serde_json::Value = serde_json::from_str(&content)?;
        if let Some(token) = parsed.get("token").and_then(|t| t.as_str()) {
            return Ok(token.to_string());
        }
    }

    // Check env var
    if let Ok(token) = std::env::var("BROOMVA_TOKEN") {
        return Ok(token);
    }

    anyhow::bail!("No auth token found. Run `broomva relay auth` first.")
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
        server_url: "https://broomva.tech".to_string(),
        bind_address: "127.0.0.1:3004".to_string(),
        node_name: hostname,
    })
}
