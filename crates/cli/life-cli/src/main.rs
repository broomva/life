//! Life CLI — Production agent deployment pipeline for Life Agent OS.
//!
//! One-command deployment of pre-configured agent stacks to Railway, Fly.io, or AWS ECS.
//!
//! Usage:
//!   life deploy --agent coding-agent --target railway
//!   life status --agent coding-agent
//!   life destroy --agent coding-agent
//!   life templates list
//!   life cost --agent coding-agent

mod cli;
mod cost;
mod deploy;
mod logs;
mod relay;
mod scale;
mod status;
mod template;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("life=info".parse()?))
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Deploy(args) => deploy::run(args).await,
        Command::Status(args) => status::run(args).await,
        Command::List(args) => deploy::list(args).await,
        Command::Destroy(args) => deploy::destroy(args).await,
        Command::Templates(args) => template::run(args),
        Command::Cost(args) => cost::run(args).await,
        Command::Logs(args) => logs::run(args).await,
        Command::Scale(args) => scale::run(args).await,
        Command::Relay { command } => relay::run(command).await,
    }
}
