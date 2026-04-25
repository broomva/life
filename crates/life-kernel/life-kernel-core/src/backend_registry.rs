//! Thread-safe registry that resolves
//! [`aios_protocol::hypervisor::BackendSelector`]s to concrete
//! [`aios_protocol::hypervisor::HypervisorBackend`] implementations.
//!
//! The engine holds one [`BackendRegistry`] per
//! `KernelEngine` instance. Registration happens at construction time
//! (usually via `KernelEngineBuilder`) and is expected to be stable for
//! the lifetime of the engine — the registry uses a
//! [`tokio::sync::RwLock`] so dynamic registration is possible but
//! typical workloads take the fast read path.
//!
//! ## Resolution semantics
//!
//! * [`aios_protocol::hypervisor::BackendSelector::Explicit`]
//!   — look up by name; miss returns [`RegistryError::BackendNotFound`].
//! * [`aios_protocol::hypervisor::BackendSelector::Auto`]
//!   — return the first registered backend whose
//!   [`capabilities`](aios_protocol::hypervisor::HypervisorBackend::capabilities)
//!   set is non-empty; empty registry or all-empty-capabilities surface
//!   as [`RegistryError::NoBackendMatches`].
//!
//! "First" is defined as insertion order through
//! [`BackendRegistry::register`]; callers that want deterministic
//! ordering should register backends in their preferred priority.
//!
//! ## Invariants
//!
//! * **No hidden state.** The registry's observable behaviour is a
//!   total function of the explicit registrations it received.
//! * **Thread-safe.** `register`, `resolve`, and `list_backend_ids` are
//!   safe to call concurrently from multiple tasks.

use std::collections::HashMap;
use std::sync::Arc;

use aios_protocol::hypervisor::{BackendId, BackendSelector, HypervisorBackend};
use tokio::sync::RwLock;

/// Errors returned by [`BackendRegistry::resolve`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// The registry has no backend registered under the requested
    /// [`BackendId`].
    #[error("backend not found: {0}")]
    BackendNotFound(BackendId),
    /// A [`BackendSelector::Auto`] request could not be satisfied
    /// because no registered backend advertises any capabilities.
    #[error("no registered backend matches the Auto selector")]
    NoBackendMatches,
    /// A selector variant added after this build of the registry was
    /// compiled. `aios_protocol::hypervisor::BackendSelector` is marked
    /// `#[non_exhaustive]`, so future variants must fall through a
    /// wildcard arm; surfacing them as a typed error makes the
    /// forward-compat story visible to callers.
    #[error("unsupported selector variant; rebuild against a newer life-kernel-core")]
    UnsupportedSelector,
}

/// Thread-safe, insertion-ordered registry of
/// [`HypervisorBackend`] implementations.
///
/// Cheap to clone — the internal state sits behind an
/// [`Arc`] so clones share the same registration table.
#[derive(Clone, Default)]
pub struct BackendRegistry {
    inner: Arc<RwLock<State>>,
}

#[derive(Default)]
struct State {
    /// Map from [`BackendId`] to the registered backend. Lookup by id
    /// is O(1).
    by_id: HashMap<BackendId, Arc<dyn HypervisorBackend>>,
    /// Insertion-ordered ids. Used to implement deterministic
    /// [`BackendSelector::Auto`] resolution without iterating the
    /// HashMap (which is unordered).
    order: Vec<BackendId>,
}

impl BackendRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `backend` under the id returned by its
    /// [`HypervisorBackend::name`] method.
    ///
    /// Re-registering the same id replaces the previous entry but
    /// preserves its position in insertion order. `register` is safe
    /// to call concurrently but callers should register at engine
    /// construction time to get deterministic
    /// [`BackendSelector::Auto`] behaviour.
    pub async fn register(&self, backend: Arc<dyn HypervisorBackend>) {
        let id = BackendId::from(backend.name());
        let mut state = self.inner.write().await;
        if !state.by_id.contains_key(&id) {
            state.order.push(id.clone());
        }
        state.by_id.insert(id, backend);
    }

    /// Resolve `selector` to a registered backend.
    ///
    /// See module docs for the selection rules.
    pub async fn resolve(
        &self,
        selector: &BackendSelector,
    ) -> Result<Arc<dyn HypervisorBackend>, RegistryError> {
        let state = self.inner.read().await;
        match selector {
            BackendSelector::Explicit { backend } => state
                .by_id
                .get(backend)
                .cloned()
                .ok_or_else(|| RegistryError::BackendNotFound(backend.clone())),
            BackendSelector::Auto => state
                .order
                .iter()
                .find_map(|id| {
                    state.by_id.get(id).and_then(|b| {
                        if b.capabilities().is_empty() {
                            None
                        } else {
                            Some(b.clone())
                        }
                    })
                })
                .ok_or(RegistryError::NoBackendMatches),
            // `BackendSelector` is `#[non_exhaustive]`; any variant we do
            // not yet recognise cannot be resolved deterministically.
            _ => Err(RegistryError::UnsupportedSelector),
        }
    }

    /// Return the ids of all registered backends in insertion order.
    ///
    /// Useful for diagnostics and for the (future) `/backends`
    /// introspection endpoint on the soma daemon.
    pub async fn list_backend_ids(&self) -> Vec<BackendId> {
        let state = self.inner.read().await;
        state.order.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aios_protocol::hypervisor::{
        BackendCapabilitySet, BackendError, ExecRequest, ExecResult, VmHandle, VmSnapshotId, VmSpec,
    };
    use async_trait::async_trait;
    use chrono::Utc;

    /// Minimal [`HypervisorBackend`] used only to exercise the registry.
    ///
    /// The `create` / `exec` / ... methods return canned values — no
    /// tests in this module drive the backend through those paths;
    /// they only care about [`HypervisorBackend::name`] and
    /// [`HypervisorBackend::capabilities`].
    struct StubBackend {
        name: &'static str,
        caps: BackendCapabilitySet,
    }

    impl StubBackend {
        fn new(name: &'static str, caps: BackendCapabilitySet) -> Arc<Self> {
            Arc::new(Self { name, caps })
        }
    }

    #[async_trait]
    impl HypervisorBackend for StubBackend {
        fn name(&self) -> &'static str {
            self.name
        }

        fn capabilities(&self) -> BackendCapabilitySet {
            self.caps
        }

        async fn create(&self, _spec: VmSpec) -> Result<VmHandle, BackendError> {
            Ok(canned_handle(self.name))
        }

        async fn exec(
            &self,
            _vm: &VmHandle,
            _req: ExecRequest,
        ) -> Result<ExecResult, BackendError> {
            Ok(ExecResult {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
                duration_ms: 0,
            })
        }

        async fn snapshot(&self, _vm: &VmHandle) -> Result<VmSnapshotId, BackendError> {
            Ok(VmSnapshotId::from("stub-snap"))
        }

        async fn restore(&self, _snapshot: &VmSnapshotId) -> Result<VmHandle, BackendError> {
            Ok(canned_handle(self.name))
        }

        async fn destroy(&self, _vm: &VmHandle) -> Result<(), BackendError> {
            Ok(())
        }
    }

    fn canned_handle(backend_name: &str) -> VmHandle {
        use aios_protocol::hypervisor::{VmId, VmStatus};
        use aios_protocol::ids::{AgentId, SessionId};
        VmHandle {
            vm_id: VmId::from("stub-vm"),
            backend: BackendId::from(backend_name),
            session_id: SessionId::from_string("stub-session"),
            agent_id: AgentId::from_string("stub-agent"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    // `Arc<dyn HypervisorBackend>` does not implement Debug, so we
    // cannot use `.expect` / `.expect_err`; match on the result
    // manually.
    fn assert_name(result: Result<Arc<dyn HypervisorBackend>, RegistryError>, expected: &str) {
        match result {
            Ok(backend) => assert_eq!(backend.name(), expected),
            Err(e) => panic!("expected backend `{expected}`, got error: {e:?}"),
        }
    }

    fn unwrap_err(result: Result<Arc<dyn HypervisorBackend>, RegistryError>) -> RegistryError {
        match result {
            Ok(backend) => panic!("expected an error, got backend `{}`", backend.name()),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn registry_registers_and_resolves_explicit() {
        let registry = BackendRegistry::new();
        let backend = StubBackend::new("local", BackendCapabilitySet::FILESYSTEM_READ);
        registry.register(backend.clone()).await;

        assert_name(
            registry
                .resolve(&BackendSelector::Explicit {
                    backend: BackendId::from("local"),
                })
                .await,
            "local",
        );
        assert_eq!(
            registry.list_backend_ids().await,
            vec![BackendId::from("local")]
        );
    }

    #[tokio::test]
    async fn registry_auto_selects_first_matching() {
        let registry = BackendRegistry::new();
        // First registration advertises no capabilities → should be
        // skipped by the Auto selector.
        registry
            .register(StubBackend::new(
                "stub-empty",
                BackendCapabilitySet::empty(),
            ))
            .await;
        registry
            .register(StubBackend::new(
                "local",
                BackendCapabilitySet::FILESYSTEM_READ,
            ))
            .await;
        // A second capable backend exists; Auto must pick the first
        // capable one in insertion order (`local`), not this one.
        registry
            .register(StubBackend::new(
                "cube",
                BackendCapabilitySet::FORK | BackendCapabilitySet::PERSISTENCE,
            ))
            .await;

        assert_name(registry.resolve(&BackendSelector::Auto).await, "local");
    }

    #[tokio::test]
    async fn registry_returns_err_when_backend_missing() {
        let registry = BackendRegistry::new();
        registry
            .register(StubBackend::new(
                "local",
                BackendCapabilitySet::FILESYSTEM_READ,
            ))
            .await;

        let err = unwrap_err(
            registry
                .resolve(&BackendSelector::Explicit {
                    backend: BackendId::from("missing"),
                })
                .await,
        );

        match err {
            RegistryError::BackendNotFound(id) => {
                assert_eq!(id, BackendId::from("missing"));
            }
            other => panic!("expected BackendNotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn registry_returns_err_when_no_backend_matches_auto() {
        let registry = BackendRegistry::new();

        // Empty registry → NoBackendMatches.
        assert!(matches!(
            unwrap_err(registry.resolve(&BackendSelector::Auto).await),
            RegistryError::NoBackendMatches
        ));

        // Non-empty, but every backend advertises zero capabilities.
        registry
            .register(StubBackend::new(
                "stub-empty",
                BackendCapabilitySet::empty(),
            ))
            .await;
        assert!(matches!(
            unwrap_err(registry.resolve(&BackendSelector::Auto).await),
            RegistryError::NoBackendMatches
        ));
    }
}
