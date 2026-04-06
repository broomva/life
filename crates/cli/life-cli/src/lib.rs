//! Life CLI — library entrypoint for embedding in life-os.

pub mod cli;
pub mod cost;
pub mod deploy;
pub mod logs;
pub mod relay;
pub mod scale;
pub mod setup;
pub mod status;
pub mod template;

use anyhow::Result;
use clap::Parser;

/// Run the Life CLI with the given args (or from std::env).
pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("life=info".parse()?),
        )
        .init();

    let cli = cli::Cli::parse();

    match cli.command {
        None => {
            setup::print_quick_help();
            Ok(())
        }
        Some(cmd) => match cmd {
            cli::Command::Setup => setup::run().await,
            cli::Command::Deploy(args) => deploy::run(args).await,
            cli::Command::Status(args) => status::run(args).await,
            cli::Command::List(args) => deploy::list(args).await,
            cli::Command::Destroy(args) => deploy::destroy(args).await,
            cli::Command::Templates(args) => template::run(args),
            cli::Command::Cost(args) => cost::run(args).await,
            cli::Command::Logs(args) => logs::run(args).await,
            cli::Command::Scale(args) => scale::run(args).await,
            cli::Command::Relay { command } => relay::run(command).await,
        },
    }
}
