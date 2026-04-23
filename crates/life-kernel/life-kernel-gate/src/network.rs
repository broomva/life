//! Permissive network-isolation gate: `apply` is a no-op; `record_egress`
//! accumulates into an atomic counter for conformance assertions.
//!
//! Real egress filtering — first a userspace allow-list, then an eBPF
//! enforcement path — lands in Phase 4 (see the spec §6).

use std::sync::atomic::{AtomicU64, Ordering};

use aios_protocol::hypervisor::VmHandle;
use aios_protocol::kernel::KernelResult;
use aios_protocol::network_isolation::{EgressTarget, NetworkIsolationPort};
use aios_protocol::sandbox::NetworkPolicy;
use async_trait::async_trait;

/// [`NetworkIsolationPort`] impl that enforces nothing but records
/// cumulative egress bytes observed across every VM it is applied to.
///
/// [`apply`](NetworkIsolationPort::apply) is a no-op that simply returns
/// `Ok(())` — Phase 1 ships this as the default wiring so the kernel
/// engine's VM-bring-up path compiles end-to-end without depending on
/// a real eBPF stack.
///
/// [`record_egress`](NetworkIsolationPort::record_egress) accumulates
/// the `bytes` argument into an internal
/// [`AtomicU64`] so conformance suites can
/// call [`egress_bytes_total`](NoOpNetworkIsolation::egress_bytes_total)
/// to assert that the engine wired the recording call correctly.
#[derive(Debug, Default)]
pub struct NoOpNetworkIsolation {
    egress_bytes: AtomicU64,
}

impl NoOpNetworkIsolation {
    /// Construct a fresh gate with a zeroed egress counter. Equivalent
    /// to [`NoOpNetworkIsolation::default`].
    pub const fn new() -> Self {
        Self {
            egress_bytes: AtomicU64::new(0),
        }
    }

    /// Cumulative egress bytes observed across all VMs since
    /// construction.
    ///
    /// Conformance suites read this counter to verify that the engine
    /// invoked [`NetworkIsolationPort::record_egress`] for every
    /// observed flow. The counter is monotonically increasing and only
    /// reset by dropping the gate.
    pub fn egress_bytes_total(&self) -> u64 {
        self.egress_bytes.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl NetworkIsolationPort for NoOpNetworkIsolation {
    async fn apply(&self, _vm: &VmHandle, _policy: &NetworkPolicy) -> KernelResult<()> {
        Ok(())
    }

    async fn record_egress(
        &self,
        _vm: &VmHandle,
        bytes: u64,
        _dst: &EgressTarget,
    ) -> KernelResult<()> {
        self.egress_bytes.fetch_add(bytes, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use aios_protocol::hypervisor::{BackendId, VmId, VmStatus};
    use aios_protocol::ids::{AgentId, SessionId};
    use aios_protocol::network_isolation::EgressProtocol;
    use chrono::Utc;

    /// Build a minimal [`VmHandle`] used to drive the isolation port in
    /// tests. Field values are only meaningful to the extent that the
    /// handle type-checks — the NoOp gate ignores every field.
    fn vm_handle() -> VmHandle {
        VmHandle {
            vm_id: VmId::from("vm-noop-net"),
            backend: BackendId::from("stub"),
            session_id: SessionId::from_string("sess-noop-net"),
            agent_id: AgentId::from_string("agent-noop-net"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    /// Canonical egress target for the accumulator tests.
    fn target() -> EgressTarget {
        EgressTarget {
            host: "example.test".into(),
            port: 443,
            protocol: EgressProtocol::Tcp,
        }
    }

    #[tokio::test]
    async fn noop_network_apply_always_ok() {
        let gate = NoOpNetworkIsolation::new();
        let vm = vm_handle();
        // Drive every variant of NetworkPolicy to lock in that the gate
        // never rejects a declared policy.
        assert!(gate.apply(&vm, &NetworkPolicy::Disabled).await.is_ok());
        assert!(gate.apply(&vm, &NetworkPolicy::AllowAll).await.is_ok());
        assert!(
            gate.apply(
                &vm,
                &NetworkPolicy::AllowList {
                    hosts: vec!["example.test".into()],
                },
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn noop_network_record_egress_accumulates() {
        let gate = NoOpNetworkIsolation::default();
        let vm = vm_handle();
        let dst = target();
        assert_eq!(gate.egress_bytes_total(), 0);

        gate.record_egress(&vm, 10, &dst).await.unwrap();
        gate.record_egress(&vm, 200, &dst).await.unwrap();
        gate.record_egress(&vm, 3_000, &dst).await.unwrap();

        assert_eq!(gate.egress_bytes_total(), 3_210);
    }

    #[tokio::test]
    async fn noop_network_record_egress_atomic_counter_monotonic() {
        // Race 100 concurrent record_egress calls to verify the atomic
        // accumulator is race-free and the final total matches the
        // arithmetic sum.
        let gate = Arc::new(NoOpNetworkIsolation::new());
        let vm = Arc::new(vm_handle());
        let dst = Arc::new(target());

        let mut handles = Vec::with_capacity(100);
        let mut expected_total: u64 = 0;
        for i in 1u64..=100u64 {
            expected_total += i;
            let gate = Arc::clone(&gate);
            let vm = Arc::clone(&vm);
            let dst = Arc::clone(&dst);
            handles.push(tokio::spawn(async move {
                gate.record_egress(&vm, i, &dst).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(gate.egress_bytes_total(), expected_total);
    }

    #[tokio::test]
    async fn noop_network_is_dyn_compatible() {
        let gate: Arc<dyn NetworkIsolationPort> = Arc::new(NoOpNetworkIsolation::new());
        gate.apply(&vm_handle(), &NetworkPolicy::Disabled)
            .await
            .unwrap();
        gate.record_egress(&vm_handle(), 42, &target())
            .await
            .unwrap();
    }
}
