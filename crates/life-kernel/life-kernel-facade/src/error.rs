//! Facade error surface + canonical-error mapping.

use aios_protocol::error::KernelError;
use thiserror::Error;

/// Errors raised by the facade crate (proxies, adapters, retry).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FacadeError {
    /// Downstream daemon could not be reached (connect refused, dns,
    /// timeout, 5xx), including the daemon name for attribution.
    #[error("backend unavailable ({daemon}): {source}")]
    BackendUnavailable {
        daemon: &'static str,
        #[source]
        source: anyhow::Error,
    },

    /// Downstream daemon returned a 4xx response.
    #[error("backend rejected ({daemon}): status {status}: {message}")]
    BackendRejected {
        daemon: &'static str,
        status: u16,
        message: String,
    },

    /// Downstream daemon produced a malformed payload.
    #[error("backend protocol violation ({daemon}): {reason}")]
    BackendProtocol {
        daemon: &'static str,
        reason: String,
    },

    /// SSE / streaming body broke mid-flight.
    #[error("backend stream broken ({daemon}): {reason}")]
    BackendStreamBroken {
        daemon: &'static str,
        reason: String,
    },

    /// Happens only when the adapter explicitly declines to service a
    /// request (e.g. v0.2 stubs on a v0 deployment).
    #[error("unimplemented: {0}")]
    Unimplemented(&'static str),
}

/// Convenience result type for facade operations.
pub type FacadeResult<T> = Result<T, FacadeError>;

impl From<FacadeError> for KernelError {
    fn from(err: FacadeError) -> KernelError {
        match err {
            FacadeError::BackendUnavailable { daemon, source } => {
                // KernelError has no Internal/Unavailable variant — map to Runtime.
                KernelError::Runtime(format!("{daemon}: {source}"))
            }
            FacadeError::BackendRejected { daemon, status, message } => {
                // 4xx from daemon maps to InvalidState (closest to bad-request).
                KernelError::InvalidState(format!("{daemon} {status}: {message}"))
            }
            FacadeError::BackendProtocol { daemon, reason } => {
                KernelError::Runtime(format!("{daemon} protocol: {reason}"))
            }
            FacadeError::BackendStreamBroken { daemon, reason } => {
                KernelError::Runtime(format!("{daemon} stream: {reason}"))
            }
            FacadeError::Unimplemented(what) => {
                // No Unimplemented variant in legacy KernelError — use InvalidState.
                KernelError::InvalidState(format!("unimplemented: {what}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_maps_to_runtime() {
        let err = FacadeError::BackendUnavailable {
            daemon: "lagod",
            source: anyhow::anyhow!("connection refused"),
        };
        let kernel: KernelError = err.into();
        assert!(matches!(kernel, KernelError::Runtime(_)));
    }

    #[test]
    fn unimplemented_maps_to_invalid_state() {
        let err = FacadeError::Unimplemented("life.Tools.Execute");
        let kernel: KernelError = err.into();
        assert!(matches!(kernel, KernelError::InvalidState(_)));
    }

    #[test]
    fn backend_rejected_maps_to_invalid_state() {
        let err = FacadeError::BackendRejected {
            daemon: "arcand",
            status: 422,
            message: "unprocessable".into(),
        };
        let kernel: KernelError = err.into();
        assert!(matches!(kernel, KernelError::InvalidState(_)));
    }

    #[test]
    fn backend_protocol_maps_to_runtime() {
        let err = FacadeError::BackendProtocol {
            daemon: "lagod",
            reason: "bad json".into(),
        };
        let kernel: KernelError = err.into();
        assert!(matches!(kernel, KernelError::Runtime(_)));
    }
}
