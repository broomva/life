//! Error surface for the `lifed` daemon.

use thiserror::Error;

/// All errors surfaced by the daemon entrypoint.
///
/// The daemon converts every fallible subsystem result into this enum at the
/// `main.rs` boundary so startup failures surface with a single `LifedError`
/// printed to stderr (and to the systemd journal via the inherited stdio).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LifedError {
    /// Raised by `config::LifedConfig::load` when the config file is missing,
    /// unreadable, malformed TOML, or validates to an invalid combination
    /// (e.g. vsock listener enabled with no CID).
    #[error("configuration: {0}")]
    Config(String),

    /// Raised while instantiating a backend, gate, or event store.
    #[error("backend initialisation: {0}")]
    BackendInit(String),

    /// Raised by the tonic server or the listener accept loops.
    #[error("server: {0}")]
    Server(String),

    /// Raised while draining in-flight dispatches during shutdown.
    #[error("shutdown: {0}")]
    Shutdown(String),
}

/// Convenience alias used throughout the daemon.
pub type LifedResult<T> = Result<T, LifedError>;
