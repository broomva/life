//! Operator CLI subcommands for `soma`.
//!
//! Each submodule exposes a `pub struct Args: clap::Args` and a
//! `pub async fn run(socket: &Path, args: Args) -> anyhow::Result<()>`.

#![deny(unsafe_code)]

pub mod client;
pub mod create_vm;
pub mod dispatch;
pub mod list_vms;
