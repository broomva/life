//! Life CLI — Agent Operating System for Life Agent OS.
//!
//! Run `life` with no arguments for a branded welcome screen,
//! or `life setup` for the interactive onboarding wizard.
//!
//! Usage:
//!   life                        — show banner + quick help
//!   life setup                  — interactive setup wizard
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
mod setup;
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
        None => {
            setup::print_quick_help();
            Ok(())
        }
        Some(Command::Setup) => setup::run().await,
        Some(Command::Deploy(args)) => deploy::run(args).await,
        Some(Command::Status(args)) => status::run(args).await,
        Some(Command::List(args)) => deploy::list(args).await,
        Some(Command::Destroy(args)) => deploy::destroy(args).await,
        Some(Command::Templates(args)) => template::run(args),
        Some(Command::Cost(args)) => cost::run(args).await,
        Some(Command::Logs(args)) => logs::run(args).await,
        Some(Command::Scale(args)) => scale::run(args).await,
        Some(Command::Relay { command }) => relay::run(command).await,
    }
}
