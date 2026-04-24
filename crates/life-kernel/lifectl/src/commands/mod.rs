//! CLI subcommands for `lifectl`.

pub mod create_vm;
pub mod dispatch;
pub mod list_vms;

/// The set of subcommands exposed by `lifectl`.
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Create a new VM in the lifed daemon.
    CreateVm(create_vm::Args),
    /// Dispatch a tool call into an existing VM.
    Dispatch(dispatch::Args),
    /// List VMs managed by the daemon.
    ListVms(list_vms::Args),
}

impl Command {
    /// Dispatch the selected subcommand against the daemon socket at `socket`.
    pub async fn run(self, socket: &std::path::Path) -> anyhow::Result<()> {
        match self {
            Self::CreateVm(a) => create_vm::run(socket, a).await,
            Self::Dispatch(a) => dispatch::run(socket, a).await,
            Self::ListVms(a) => list_vms::run(socket, a).await,
        }
    }
}
