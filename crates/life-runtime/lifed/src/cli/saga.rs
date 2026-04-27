//! `lifed saga show <saga_id>` operator subcommand.
//!
//! Talks to the admin-plane `Saga` service.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use life_runtime_proto::life::admin::v1::{SagaRef, saga_client::SagaClient};

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
    let channel = crate::cli::client::connect(&args.socket).await?;
    let mut client = SagaClient::new(channel);
    let r = client
        .show(SagaRef {
            saga_id: args.saga_id,
        })
        .await?
        .into_inner();
    println!("{r:#?}");
    Ok(())
}
