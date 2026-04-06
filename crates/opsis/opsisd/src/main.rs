use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::info;
use tracing_subscriber::EnvFilter;

use opsis_engine::config::load_feeds_config;
use opsis_engine::engine::{EngineConfig, OpsisEngine};
use opsis_engine::feeds::usgs::UsgsEarthquakeFeed;
use opsis_engine::feeds::weather::OpenMeteoWeatherFeed;
use opsis_engine::registry::ClientRegistry;
use opsis_engine::stream::{AppState, build_router};

#[derive(Parser)]
#[command(name = "opsisd", about = "Opsis world state engine daemon")]
struct Cli {
    /// Server bind address.
    #[arg(long, default_value = "127.0.0.1:3010")]
    bind: String,

    /// Tick rate in Hz.
    #[arg(long, default_value = "1.0")]
    hz: f64,

    /// Path to feeds.toml configuration file.
    #[arg(long, default_value = "feeds.toml")]
    feeds_config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    info!(bind = %cli.bind, hz = cli.hz, "starting opsisd");

    // Build engine.
    let config = EngineConfig {
        hz: cli.hz,
        bind_addr: cli.bind.clone(),
    };
    let mut engine = OpsisEngine::new(config);

    // Try to load feeds from config file; fall back to hardcoded feeds.
    match load_feeds_config(&cli.feeds_config) {
        Ok(feeds_config) => {
            let count = feeds_config.feeds.len();
            // For now, we still use the hardcoded feed implementations keyed by name.
            // The feeds.toml serves as the declarative source of truth for which feeds
            // to enable; the actual FeedIngestor impl is matched by name.
            for feed_cfg in &feeds_config.feeds {
                match feed_cfg.name.as_str() {
                    "usgs-earthquake" => {
                        engine.add_feed(Box::new(UsgsEarthquakeFeed::new()));
                    }
                    "open-meteo" => {
                        engine.add_feed(Box::new(OpenMeteoWeatherFeed::new()));
                    }
                    other => {
                        tracing::warn!(name = other, "unknown feed in config — skipping");
                    }
                }
            }
            info!(
                path = %cli.feeds_config.display(),
                feeds = count,
                "loaded feeds from config"
            );
        }
        Err(e) => {
            tracing::warn!(
                path = %cli.feeds_config.display(),
                error = %e,
                "failed to load feeds config — using hardcoded defaults"
            );
            engine.add_feed(Box::new(UsgsEarthquakeFeed::new()));
            engine.add_feed(Box::new(OpenMeteoWeatherFeed::new()));
            info!("registered 2 default feeds: usgs-earthquake, open-meteo");
        }
    }

    // Shutdown signal.
    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

    // Build HTTP server.
    let app_state = AppState {
        bus: engine.bus.clone(),
        registry: ClientRegistry::new(),
        started_at: std::time::Instant::now(),
    };
    let router = build_router(app_state);

    let listener = TcpListener::bind(&cli.bind).await?;
    info!(addr = %cli.bind, "opsis HTTP server listening");

    // Run engine and server concurrently.
    tokio::select! {
        _ = engine.run(shutdown_rx) => {
            info!("engine stopped");
        }
        result = axum::serve(listener, router) => {
            if let Err(e) = result {
                tracing::error!("server error: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c, shutting down");
            let _ = shutdown_tx.send(());
        }
    }

    Ok(())
}
