//! Nous daemon — metacognitive evaluation service.
//!
//! Runs the Nous HTTP API for external eval queries and
//! async LLM-as-judge evaluations.

use clap::Parser;
use tracing::info;

/// Nous metacognitive evaluation daemon.
#[derive(Parser)]
#[command(name = "nousd", about = "Nous evaluation daemon")]
struct Cli {
    /// Bind address.
    #[arg(long, default_value = "127.0.0.1:3004")]
    bind: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    // Build score store and wire it into the API.
    let store = nous_api::ScoreStore::new();

    // Default heuristic registry has 6 evaluators.
    let registry = nous_heuristics::default_registry()?;
    let evaluator_count = registry.len() as u32;

    let app = nous_api::nous_router(store, evaluator_count);

    let listener = tokio::net::TcpListener::bind(&cli.bind).await?;
    info!(bind = %cli.bind, evaluator_count, "nousd starting");

    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_default_bind() {
        let cli = Cli::parse_from(["nousd"]);
        assert_eq!(cli.bind, "127.0.0.1:3004");
    }

    #[test]
    fn cli_custom_bind() {
        let cli = Cli::parse_from(["nousd", "--bind", "0.0.0.0:9000"]);
        assert_eq!(cli.bind, "0.0.0.0:9000");
    }
}
