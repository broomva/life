//! Life CLI — Agent Operating System.
//!
//! Run `life` with no arguments for the welcome screen,
//! or `life setup` for interactive onboarding.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    life_cli::run().await
}
