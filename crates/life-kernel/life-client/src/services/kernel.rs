//! Typed handle over `life.Kernel`.
//!
//! Kernel RPCs carry rich proto structures (`VmSpec`, `ToolCall`, etc.)
//! where a canonical aios-protocol equivalent would double the
//! conversion surface. v0 keeps the handle at the proto layer;
//! consumers that need typed access call through the existing
//! `life-kernel-proto::convert` shim.

use crate::connect::LifeClient;
use crate::error::{LifeClientError, LifeResult};
use life_kernel_proto::pb;
use pb::kernel_service_client::KernelServiceClient;

/// Typed handle over the `life.Kernel` service.
pub struct Kernel<'a> {
    client: &'a LifeClient,
}

impl<'a> Kernel<'a> {
    /// Construct a new handle. Called from `LifeClient::kernel`.
    pub(crate) fn new(client: &'a LifeClient) -> Self {
        Self { client }
    }

    /// Create a µVM per the supplied spec.
    pub async fn create_vm(&self, req: pb::CreateVmRequest) -> LifeResult<pb::VmHandle> {
        let mut c = KernelServiceClient::new(self.client.channel());
        c.create_vm(req)
            .await
            .map(|r| r.into_inner())
            .map_err(|e| LifeClientError::Rpc(e.to_string()))
    }

    /// Dispatch a Tool-ABI call into an existing µVM.
    pub async fn dispatch(&self, req: pb::DispatchRequest) -> LifeResult<pb::ToolResult> {
        let mut c = KernelServiceClient::new(self.client.channel());
        c.dispatch(req)
            .await
            .map(|r| r.into_inner())
            .map_err(|e| LifeClientError::Rpc(e.to_string()))
    }

    /// Destroy a running µVM.
    pub async fn destroy(&self, req: pb::DestroyRequest) -> LifeResult<()> {
        let mut c = KernelServiceClient::new(self.client.channel());
        c.destroy(req)
            .await
            .map(|_| ())
            .map_err(|e| LifeClientError::Rpc(e.to_string()))
    }
}
