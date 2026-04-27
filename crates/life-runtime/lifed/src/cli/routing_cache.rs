//! `lifed routing-cache dump` / `lifed routing-cache evict <sid>` operator
//! subcommands.
//!
//! Talks to the admin-plane `RoutingCache` service.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use aios_proto::aios::v1 as aios_v1;
use life_runtime_proto::life::admin::v1::{
    DumpReq, EvictReq, routing_cache_client::RoutingCacheClient,
};

#[derive(Debug, Args)]
pub struct DumpArgs {
    #[arg(
        long,
        env = "LIFED_ADMIN_SOCKET",
        default_value = "/run/life/life-admin.sock"
    )]
    pub socket: PathBuf,
    /// Maximum number of routing entries to dump. 0 means unlimited.
    #[arg(long, default_value_t = 1000)]
    pub limit: u32,
}

pub async fn run_dump(args: DumpArgs) -> Result<()> {
    let channel = crate::cli::client::connect(&args.socket).await?;
    let mut client = RoutingCacheClient::new(channel);
    let mut stream = client
        .dump(DumpReq { limit: args.limit })
        .await?
        .into_inner();
    while let Some(e) = stream.message().await? {
        let sid = e.sid.map(|s| s.value).unwrap_or_default();
        println!(
            "{} -> arcan={} lago={} haima={} anima={} (tabs={})",
            sid,
            e.arcan_addr,
            e.lago_namespace,
            e.haima_wallet,
            e.anima_account,
            e.attached_streams,
        );
    }
    Ok(())
}

#[derive(Debug, Args)]
pub struct EvictArgs {
    #[arg(
        long,
        env = "LIFED_ADMIN_SOCKET",
        default_value = "/run/life/life-admin.sock"
    )]
    pub socket: PathBuf,
    pub sid: String,
    /// Optional reason, recorded in the admin-plane log.
    #[arg(long, default_value = "operator-evict")]
    pub reason: String,
}

pub async fn run_evict(args: EvictArgs) -> Result<()> {
    let channel = crate::cli::client::connect(&args.socket).await?;
    let mut client = RoutingCacheClient::new(channel);
    client
        .evict(EvictReq {
            sid: Some(aios_v1::SessionId {
                value: args.sid.clone(),
            }),
            reason: args.reason,
        })
        .await?;
    println!("evicted {}", args.sid);
    Ok(())
}
