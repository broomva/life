//! Proto ↔ canonical DTO conversions. Wire types in `life-kernel-proto`
//! carry `bytes *_json` fields (see proto files). This module centralises
//! the serde round-trip so service impls stay tight.

use tonic::Status;

/// Deserialise a `bytes` field from proto into a canonical DTO.
pub fn from_json<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    field: &'static str,
) -> Result<T, Status> {
    serde_json::from_slice(bytes)
        .map_err(|e| Status::invalid_argument(format!("{field}: {e}")))
}

/// Serialise a canonical DTO into a `Vec<u8>` suitable for a proto `bytes` field.
pub fn to_json<T: serde::Serialize>(
    value: &T,
    field: &'static str,
) -> Result<Vec<u8>, Status> {
    serde_json::to_vec(value)
        .map_err(|e| Status::internal(format!("{field}: {e}")))
}

/// Map a `KernelError` (legacy `aios_protocol::error::KernelError`) to a
/// tonic `Status` for the wire response.
pub fn kernel_err_to_status(err: aios_protocol::error::KernelError) -> Status {
    use aios_protocol::error::KernelError;
    match err {
        KernelError::CapabilityDenied(m) => Status::permission_denied(m),
        KernelError::ToolNotFound(m) => Status::not_found(m),
        KernelError::ApprovalRequired(m) => Status::failed_precondition(m),
        KernelError::Io(m) => Status::internal(m),
        KernelError::Serialization(m) => Status::internal(format!("serialization: {m}")),
        KernelError::InvalidState(m) => Status::invalid_argument(m),
        KernelError::Runtime(m) => Status::internal(m),
        KernelError::BudgetExceeded(m) => Status::resource_exhausted(m),
        KernelError::SequenceConflict { expected, actual } => {
            Status::aborted(format!("sequence conflict: expected {expected}, got {actual}"))
        }
        // Non-exhaustive — handle future variants defensively.
        #[allow(unreachable_patterns)]
        other => Status::internal(format!("{other:?}")),
    }
}
