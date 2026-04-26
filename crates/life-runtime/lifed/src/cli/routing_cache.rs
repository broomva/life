//! `lifed routing-cache dump` operator subcommand.
//!
//! Sub-phase A: scaffold only. Sub-phase C wires it against the admin-plane
//! `RoutingCache` service.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct DumpArgs {
    #[arg(
        long,
        env = "LIFED_ADMIN_SOCKET",
        default_value = "/run/life/life-admin.sock"
    )]
    pub socket: PathBuf,
}

pub async fn run_dump(args: DumpArgs) -> Result<()> {
    eprintln!("lifed routing-cache dump — sub-phase C wires this");
    eprintln!("(socket: {})", args.socket.display());
    Ok(())
}
