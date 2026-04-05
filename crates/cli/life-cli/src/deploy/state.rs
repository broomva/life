//! Deployment state persistence — tracks deployed agents locally.
//!
//! State is saved to ~/.life/deployments/{agent-name}.json so that
//! `life status` and `life destroy` can reference previous deployments.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::backend::DeployedService;

/// Persisted state of a deployed agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentState {
    pub agent_name: String,
    pub project_name: String,
    pub target: String,
    pub project_id: String,
    pub environment_id: String,
    pub services: HashMap<String, DeployedService>,
    pub deployed_at: DateTime<Utc>,
    pub template_name: String,
}

impl DeploymentState {
    /// Directory where deployment states are stored.
    fn state_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        let dir = home.join(".life").join("deployments");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        Ok(dir)
    }

    /// Path for a specific agent's state file.
    fn state_path(agent_name: &str) -> Result<PathBuf> {
        Ok(Self::state_dir()?.join(format!("{agent_name}.json")))
    }

    /// Save this deployment state to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::state_path(&self.agent_name)?;
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Load a deployment state from disk.
    pub fn load(agent_name: &str) -> Result<Self> {
        let path = Self::state_path(agent_name)?;
        let json = std::fs::read_to_string(&path)
            .with_context(|| format!("no deployment state found at {}", path.display()))?;
        serde_json::from_str(&json).context("failed to parse deployment state")
    }

    /// Remove the state file after destroy.
    pub fn remove(&self) -> Result<()> {
        let path = Self::state_path(&self.agent_name)?;
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// List all saved deployment states.
    #[allow(dead_code)]
    pub fn list_all() -> Result<Vec<Self>> {
        let dir = Self::state_dir()?;
        let mut states = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    if let Ok(json) = std::fs::read_to_string(&path) {
                        if let Ok(state) = serde_json::from_str::<DeploymentState>(&json) {
                            states.push(state);
                        }
                    }
                }
            }
        }

        Ok(states)
    }
}
