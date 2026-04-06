//! life-os — Install with `cargo install life-os` to get the `life` command.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    life_cli::run().await
}
