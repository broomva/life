//! `lifed` — Life Runtime facade-aggregator daemon.

#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "lifed",
    version,
    about = "Life Runtime facade-aggregator daemon — public + admin plane RPCs",
    long_about = "Hosts the locked life.v1.* and life.admin.v1.* surfaces over UDS,\n\
                  fans out to per-substrate daemons via per-substrate sockets,\n\
                  validates Tier-2 capability tokens and mints Tier-3 substrate tokens."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the daemon (default mode for systemd unit `lifed.service`).
    Daemon {
        #[arg(long, env = "LIFED_CONFIG")]
        config: Option<PathBuf>,
        /// Sub-phase D: opt into the dev mock-substrate fallback when
        /// the real substrate UDS sockets are missing. Production
        /// deployments leave this off; lifed fails fast on missing
        /// sockets instead of silently running on mocks.
        #[arg(long, env = "LIFED_ALLOW_MOCK_FALLBACK", default_value_t = false)]
        allow_mock_fallback: bool,
    },

    /// Operator subcommand — list active sessions on the admin plane.
    SessionsLs(lifed::cli::sessions::LsArgs),

    /// Operator subcommand — show one session's routing entry.
    SessionsShow(lifed::cli::sessions::ShowArgs),

    /// Operator subcommand — dump the routing cache.
    RoutingCacheDump(lifed::cli::routing_cache::DumpArgs),

    /// Operator subcommand — force-evict one routing-cache entry.
    RoutingCacheEvict(lifed::cli::routing_cache::EvictArgs),

    /// Operator subcommand — show one saga.
    SagaShow(lifed::cli::saga::ShowArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Daemon {
            config,
            allow_mock_fallback,
        } => {
            lifed::bootstrap::run_daemon(config.as_deref(), allow_mock_fallback).await?;
            Ok(())
        }
        Cmd::SessionsLs(args) => lifed::cli::sessions::run_ls(args).await,
        Cmd::SessionsShow(args) => lifed::cli::sessions::run_show(args).await,
        Cmd::RoutingCacheDump(args) => lifed::cli::routing_cache::run_dump(args).await,
        Cmd::RoutingCacheEvict(args) => lifed::cli::routing_cache::run_evict(args).await,
        Cmd::SagaShow(args) => lifed::cli::saga::run_show(args).await,
    }
}
