//! `lifed sessions ls` / `lifed sessions show <sid>` operator subcommands.
//!
//! Talks to the admin-plane `Runtime` + `RoutingCache` services. The
//! daemon must be running and the operator's primary GID must match the
//! daemon's `unix_socket_group` (default `life-admin`).

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use life_runtime_proto::life::admin::v1::{
    HealthReq, ListAllReq, routing_cache_client::RoutingCacheClient, runtime_client::RuntimeClient,
};

#[derive(Debug, Args)]
pub struct LsArgs {
    #[arg(
        long,
        env = "LIFED_ADMIN_SOCKET",
        default_value = "/run/life/life-admin.sock"
    )]
    pub socket: PathBuf,

    /// Maximum number of sessions to list. 0 means unlimited.
    #[arg(long, default_value_t = 100)]
    pub limit: u32,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    #[arg(
        long,
        env = "LIFED_ADMIN_SOCKET",
        default_value = "/run/life/life-admin.sock"
    )]
    pub socket: PathBuf,
    pub sid: String,
}

pub async fn run_ls(args: LsArgs) -> Result<()> {
    let channel = crate::cli::client::connect(&args.socket).await?;
    let mut client = RuntimeClient::new(channel);
    let _ = client.health_check(HealthReq {}).await?;
    let mut stream = client
        .sessions_list_all(ListAllReq { limit: args.limit })
        .await?
        .into_inner();
    println!(
        "{:<26} {:<16} {:<24} {:<10} {:>5}",
        "SID", "USER", "PROJECT", "STATUS", "TABS"
    );
    while let Some(s) = stream.message().await? {
        let sid = s.sid.map(|s| s.value).unwrap_or_default();
        println!(
            "{:<26} {:<16} {:<24} {:<10} {:>5}",
            sid, s.user_id, s.project_id, s.status, s.attached_streams
        );
    }
    Ok(())
}

pub async fn run_show(args: ShowArgs) -> Result<()> {
    // Sub-phase C: the admin Runtime service doesn't yet expose a
    // single-sid lookup, so we filter the routing cache dump
    // client-side. D6 adds Runtime.SessionShow.
    use life_runtime_proto::life::admin::v1::DumpReq;
    let channel = crate::cli::client::connect(&args.socket).await?;
    let mut client = RoutingCacheClient::new(channel);
    let mut stream = client.dump(DumpReq { limit: 100_000 }).await?.into_inner();
    while let Some(e) = stream.message().await? {
        if e.sid.as_ref().map(|s| &s.value) == Some(&args.sid) {
            println!("{e:#?}");
            return Ok(());
        }
    }
    eprintln!("session {} not found in routing cache", args.sid);
    Ok(())
}
