//! CLI argument definitions for `life`.

use clap::{Parser, Subcommand};

use crate::relay::RelayCommand;

#[derive(Parser)]
#[command(
    name = "life",
    about = "Agent Operating System — deploy, configure, and manage agents",
    version,
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize a .life/ directory in the current project.
    Init,

    /// Interactive setup wizard — configure providers, keys, and modules.
    Setup,

    /// Deploy an agent to a cloud target.
    Deploy(DeployArgs),

    /// Check the status of a deployed agent.
    Status(StatusArgs),

    /// List all deployed agents.
    List(ListArgs),

    /// Tear down a deployed agent and all its services.
    Destroy(DestroyArgs),

    /// Manage agent templates.
    Templates(TemplateArgs),

    /// Show cost tracking for a deployed agent.
    Cost(CostArgs),

    /// Stream logs from a deployed agent's services.
    Logs(LogsArgs),

    /// Scale agent services based on Autonomic economic modes.
    Scale(ScaleArgs),

    /// Manage the relay daemon for remote agent sessions.
    Relay {
        #[command(subcommand)]
        command: RelayCommand,
    },
}

#[derive(clap::Args)]
pub struct DeployArgs {
    /// Agent template name (e.g., coding-agent, data-agent, support-agent).
    #[arg(long, short)]
    pub agent: String,

    /// Deployment target (railway, flyio, ecs).
    #[arg(long, short, default_value = "railway")]
    pub target: String,

    /// LLM provider for the agent runtime (anthropic, openai, mock).
    #[arg(long, default_value = "anthropic")]
    pub provider: String,

    /// Custom project name override (defaults to life-{agent}).
    #[arg(long)]
    pub project_name: Option<String>,

    /// Path to custom template file (overrides built-in templates).
    #[arg(long)]
    pub template_path: Option<String>,

    /// Skip health check polling after deployment.
    #[arg(long)]
    pub no_wait: bool,

    /// Environment variables to pass to all services (KEY=VALUE).
    #[arg(long, short = 'e')]
    pub env: Vec<String>,
}

#[derive(clap::Args)]
pub struct StatusArgs {
    /// Agent name or Railway project ID.
    #[arg(long, short)]
    pub agent: String,

    /// Deployment target.
    #[arg(long, short, default_value = "railway")]
    pub target: String,

    /// Output format (table, json).
    #[arg(long, default_value = "table")]
    pub format: String,
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// Output format (table, json).
    #[arg(long, default_value = "table")]
    pub format: String,
}

#[derive(clap::Args)]
pub struct DestroyArgs {
    /// Agent name or Railway project ID.
    #[arg(long, short)]
    pub agent: String,

    /// Deployment target.
    #[arg(long, short, default_value = "railway")]
    pub target: String,

    /// Skip confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(clap::Args)]
pub struct TemplateArgs {
    #[command(subcommand)]
    pub command: TemplateCommand,
}

#[derive(Subcommand)]
pub enum TemplateCommand {
    /// List available agent templates.
    List,
    /// Show details of a specific template.
    Show {
        /// Template name.
        name: String,
    },
}

#[derive(clap::Args)]
pub struct CostArgs {
    /// Agent name.
    #[arg(long, short)]
    pub agent: String,

    /// Deployment target.
    #[arg(long, short, default_value = "railway")]
    pub target: String,

    /// Time window for cost data (1h, 24h, 7d, 30d).
    #[arg(long, default_value = "24h")]
    pub window: String,

    /// Output format (table, json).
    #[arg(long, default_value = "table")]
    pub format: String,
}

#[derive(clap::Args)]
pub struct LogsArgs {
    /// Agent name.
    #[arg(long, short)]
    pub agent: String,

    /// Specific service to stream logs from (streams all if omitted).
    #[arg(long, short)]
    pub service: Option<String>,

    /// Deployment target.
    #[arg(long, short, default_value = "railway")]
    pub target: String,

    /// Number of recent log lines to show (0 = stream live only).
    #[arg(long, short, default_value = "50")]
    pub lines: u32,
}

#[derive(clap::Args)]
pub struct ScaleArgs {
    /// Agent name.
    #[arg(long, short)]
    pub agent: String,

    /// Service to scale (scales arcan by default).
    #[arg(long, short, default_value = "arcan")]
    pub service: String,

    /// Number of replicas (overrides auto-scaling).
    #[arg(long, short)]
    pub replicas: Option<u32>,

    /// Use Autonomic economic mode to determine scale.
    /// Queries the Autonomic service and scales based on its recommendation.
    #[arg(long)]
    pub auto: bool,

    /// Deployment target.
    #[arg(long, short, default_value = "railway")]
    pub target: String,
}
