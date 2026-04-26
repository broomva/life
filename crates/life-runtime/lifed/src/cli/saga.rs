//! `lifed saga show <saga_id>` operator subcommand.
//!
//! Sub-phase A: scaffold only. Sub-phase C wires it against the admin-plane
//! `Saga` service.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct ShowArgs {
    #[arg(
        long,
        env = "LIFED_ADMIN_SOCKET",
        default_value = "/run/life/life-admin.sock"
    )]
    pub socket: PathBuf,
    pub saga_id: String,
}

pub async fn run_show(args: ShowArgs) -> Result<()> {
    eprintln!("lifed saga show {} — sub-phase C wires this", args.saga_id);
    eprintln!("(socket: {})", args.socket.display());
    Ok(())
}
