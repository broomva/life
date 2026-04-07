use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use lago_core::id::{BranchId, SessionId};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::info;
use tracing_subscriber::EnvFilter;

use opsis_core::feed::ConnectorConfig;
use opsis_engine::config::load_feeds_config;
use opsis_engine::engine::{EngineConfig, OpsisEngine};
use opsis_engine::feeds::usgs::UsgsEarthquakeFeed;
use opsis_engine::feeds::weather::OpenMeteoWeatherFeed;
use opsis_engine::registry::ClientRegistry;
use opsis_engine::schema_registry::SchemaRegistry;
use opsis_engine::stream::{AppState, build_router};
use opsis_lago::{OpsisEventWriter, OpsisReplay};

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

    /// Lago data directory for event persistence. When set, opsisd
    /// persists all events to a local RedbJournal and replays on startup.
    #[arg(long, env = "OPSIS_LAGO_DATA_DIR")]
    lago_data_dir: Option<PathBuf>,
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

    // ── Lago persistence (optional) ────────────────────────────────
    if let Some(ref data_dir) = cli.lago_data_dir {
        std::fs::create_dir_all(data_dir)?;
        let journal_path = data_dir.join("opsis-journal.redb");
        let journal = Arc::new(
            lago_journal::RedbJournal::open(&journal_path)
                .map_err(|e| anyhow::anyhow!("failed to open Lago journal: {e}"))?,
        );

        let session_id = SessionId::from_string("opsis-world");
        let branch_id = BranchId::from("main");

        // Replay persisted events on startup.
        let replay = OpsisReplay::new(journal.clone(), session_id.clone(), branch_id.clone());
        match replay.load_events().await {
            Ok(events) if !events.is_empty() => {
                let count = events.len();
                engine.replay_events(events);
                info!(events = count, "restored world state from Lago journal");
            }
            Ok(_) => {
                info!("no persisted events found — starting fresh");
            }
            Err(e) => {
                tracing::warn!(error = %e, "replay failed — starting fresh");
            }
        }

        // Spawn background event writer.
        let writer = OpsisEventWriter::spawn(journal, session_id, branch_id, 1024);
        engine.set_persist_fn(Box::new(move |events| {
            writer.send_batch(events.iter().cloned());
        }));

        info!(path = %journal_path.display(), "opsis-lago persistence enabled");
    }

    // Try to load feeds from config file; fall back to hardcoded feeds.
    match load_feeds_config(&cli.feeds_config) {
        Ok(feeds_config) => {
            let count = feeds_config.feeds.len();

            for feed_cfg in &feeds_config.feeds {
                // Agent stream feeds use POST /events/inject — no pull ingestor.
                if matches!(feed_cfg.connector, ConnectorConfig::AgentStream { .. }) {
                    info!(name = %feed_cfg.name, "registered agent_stream feed (inject-mode)");
                    continue;
                }

                // Use the feed factory to build the ingestor.
                match opsis_engine::build_feed(feed_cfg) {
                    Ok(ingestor) => {
                        info!(name = %feed_cfg.name, schema = %feed_cfg.schema, "registered feed");
                        engine.add_feed(ingestor);
                    }
                    Err(e) => {
                        tracing::warn!(
                            name = %feed_cfg.name,
                            error = %e,
                            "failed to build feed — skipping"
                        );
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
        schema_registry: Arc::new(SchemaRegistry::new()),
        snapshot: engine.snapshot.clone(),
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
