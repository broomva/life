//! `lifed sessions ls` / `lifed sessions show <sid>` operator subcommands.
//!
//! Sub-phase A: scaffold only. Sub-phase C wires them against the admin-plane
//! `Runtime` service.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct LsArgs {
    #[arg(
        long,
        env = "LIFED_ADMIN_SOCKET",
        default_value = "/run/life/life-admin.sock"
    )]
    pub socket: PathBuf,
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
    eprintln!("lifed sessions ls — sub-phase C wires this against the admin plane");
    eprintln!("(socket: {})", args.socket.display());
    Ok(())
}

pub async fn run_show(args: ShowArgs) -> Result<()> {
    eprintln!("lifed sessions show {} — sub-phase C wires this", args.sid);
    eprintln!("(socket: {})", args.socket.display());
    Ok(())
}
