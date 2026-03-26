//! Agent template loading, validation, and listing.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tabled::{Table, Tabled};

use crate::cli::{TemplateArgs, TemplateCommand};

/// A service within an agent template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDef {
    /// Container image (e.g., ghcr.io/broomva/arcan:latest).
    pub image: String,
    /// Port the service listens on.
    pub port: u16,
    /// Whether to expose a public domain.
    #[serde(default = "default_true")]
    pub public: bool,
    /// Health check endpoint path.
    #[serde(default = "default_health_path")]
    pub health_path: String,
    /// Environment variables specific to this service.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Startup command override (uses container default if empty).
    #[serde(default)]
    pub command: Option<String>,
    /// Volume mount path for persistent data.
    #[serde(default)]
    pub volume: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_health_path() -> String {
    "/health".to_string()
}

/// An agent template — a pre-configured stack of Life services.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    /// Template metadata.
    pub meta: TemplateMeta,
    /// Services composing this agent.
    pub services: HashMap<String, ServiceDef>,
    /// Shared environment variables applied to all services.
    #[serde(default)]
    pub shared_env: HashMap<String, String>,
    /// Autonomic scaling configuration.
    #[serde(default)]
    pub scaling: ScalingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateMeta {
    /// Human-readable name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Use case this template is designed for.
    pub use_case: String,
    /// Version.
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// Autonomic-driven scaling configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingConfig {
    /// Minimum replicas.
    #[serde(default = "default_min_replicas")]
    pub min_replicas: u32,
    /// Maximum replicas.
    #[serde(default = "default_max_replicas")]
    pub max_replicas: u32,
    /// Economic mode that triggers scale-down.
    #[serde(default = "default_scale_down_mode")]
    pub scale_down_mode: String,
    /// Economic mode that triggers scale-up.
    #[serde(default = "default_scale_up_mode")]
    pub scale_up_mode: String,
}

fn default_min_replicas() -> u32 {
    1
}
fn default_max_replicas() -> u32 {
    3
}
fn default_scale_down_mode() -> String {
    "conserving".to_string()
}
fn default_scale_up_mode() -> String {
    "sovereign".to_string()
}

impl Default for ScalingConfig {
    fn default() -> Self {
        Self {
            min_replicas: default_min_replicas(),
            max_replicas: default_max_replicas(),
            scale_down_mode: default_scale_down_mode(),
            scale_up_mode: default_scale_up_mode(),
        }
    }
}

// ── Built-in templates ──────────────────────────────────────────────────────

const CODING_AGENT_TOML: &str = include_str!("../templates/coding-agent.toml");
const DATA_AGENT_TOML: &str = include_str!("../templates/data-agent.toml");
const SUPPORT_AGENT_TOML: &str = include_str!("../templates/support-agent.toml");

/// Load a template by name — checks built-in templates first, then custom path.
pub fn load_template(name: &str, custom_path: Option<&str>) -> Result<AgentTemplate> {
    // Check custom path first
    if let Some(path) = custom_path {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read template file: {path}"))?;
        return toml::from_str(&content)
            .with_context(|| format!("failed to parse template file: {path}"));
    }

    // Check built-in templates
    let toml_str = match name {
        "coding-agent" => CODING_AGENT_TOML,
        "data-agent" => DATA_AGENT_TOML,
        "support-agent" => SUPPORT_AGENT_TOML,
        _ => {
            // Try loading from ~/.life/templates/{name}.toml
            let home = dirs::home_dir().context("cannot determine home directory")?;
            let path = home.join(".life").join("templates").join(format!("{name}.toml"));
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                return toml::from_str(&content)
                    .with_context(|| format!("failed to parse {}", path.display()));
            }
            anyhow::bail!(
                "unknown template '{name}'. Available: coding-agent, data-agent, support-agent.\n\
                 Custom templates: place TOML files in ~/.life/templates/ or use --template-path."
            );
        }
    };

    toml::from_str(toml_str).with_context(|| format!("failed to parse built-in template: {name}"))
}

/// List all available templates (built-in + custom).
pub fn list_templates() -> Vec<AgentTemplate> {
    let mut templates = vec![];

    for toml_str in [CODING_AGENT_TOML, DATA_AGENT_TOML, SUPPORT_AGENT_TOML] {
        if let Ok(t) = toml::from_str::<AgentTemplate>(toml_str) {
            templates.push(t);
        }
    }

    // Scan ~/.life/templates/ for custom templates
    if let Some(home) = dirs::home_dir() {
        let custom_dir = home.join(".life").join("templates");
        if custom_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(custom_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "toml") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if let Ok(t) = toml::from_str::<AgentTemplate>(&content) {
                                templates.push(t);
                            }
                        }
                    }
                }
            }
        }
    }

    templates
}

// ── CLI handler ─────────────────────────────────────────────────────────────

#[derive(Tabled)]
struct TemplateRow {
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Description")]
    description: String,
    #[tabled(rename = "Services")]
    services: String,
    #[tabled(rename = "Use Case")]
    use_case: String,
}

pub fn run(args: TemplateArgs) -> Result<()> {
    match args.command {
        TemplateCommand::List => {
            let templates = list_templates();
            if templates.is_empty() {
                println!("No templates found.");
                return Ok(());
            }

            let rows: Vec<TemplateRow> = templates
                .iter()
                .map(|t| {
                    let svc_names: Vec<&str> = t.services.keys().map(String::as_str).collect();
                    TemplateRow {
                        name: t.meta.name.clone(),
                        description: t.meta.description.clone(),
                        services: svc_names.join(", "),
                        use_case: t.meta.use_case.clone(),
                    }
                })
                .collect();

            println!("{}", Table::new(rows));
            Ok(())
        }
        TemplateCommand::Show { name } => {
            let template = load_template(&name, None)?;
            println!("{}", toml::to_string_pretty(&template)?);
            Ok(())
        }
    }
}
