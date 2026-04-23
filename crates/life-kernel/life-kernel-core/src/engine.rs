//! [`KernelEngine`] — in-process [`KernelPort`] implementation composing
//! [`BackendRegistry`] + [`GateChain`] + [`MeteringWrapper`] on top of
//! any [`HypervisorBackend`] registered at construction.
//!
//! This is the canonical surface the daemon (`lifed`) and the in-process
//! Life Runtime library share. The engine is itself a library: the
//! daemon wraps it behind a ttrpc transport (`life-kernel-proto`),
//! while the library case uses it directly through
//! [`aios_protocol::ports::KernelPort`].
//!
//! ## Event contract
//!
//! Every [`KernelPort`] method that produces an observable state
//! transition emits exactly one `kernel.*` event through the injected
//! [`EventEmitter`]. The [`MeteringWrapper`] adds the dispatch
//! Started/Completed/UsageRecorded trio inside `dispatch`. Together
//! the engine's observable behaviour is a pure function of the event
//! journal — the foundation of the Phase 1 replay determinism work
//! (BRO-876).
//!
//! ## Ownership
//!
//! Collaborators are held as `Arc`s so a single engine can back
//! arbitrarily many concurrent dispatches. The engine itself is
//! shared-ref only: every [`KernelPort`] method takes `&self`.

use std::collections::BTreeMap;
use std::sync::Arc;

use aios_protocol::budget::{BudgetGatePort, ResourceUsage, UsageConfidence};
use aios_protocol::event::{
    EventKind, KernelDispatchDenied, KernelForkDenied, KernelVmCreated, KernelVmDestroyed,
    KernelVmForked, KernelVmHibernated, KernelVmResumed, KernelVmSnapshotted,
};
use aios_protocol::hypervisor::{
    BackendId, BackendSelector, ForkSpec, HypervisorBackend, RuntimeHint, VmHandle, VmId,
    VmSnapshotHandle, VmSnapshotId, VmSpec,
};
use aios_protocol::ids::{AgentId, BranchId, SessionId};
use aios_protocol::kernel::{GateKind, KernelContext, KernelError, KernelResult};
use aios_protocol::network_isolation::NetworkIsolationPort;
use aios_protocol::ports::{EventStorePort, KernelPort};
use aios_protocol::sandbox::NetworkPolicy;
use aios_protocol::tool::{ToolCall, ToolResult};
use async_trait::async_trait;
use chrono::Utc;

use crate::backend_registry::BackendRegistry;
use crate::dispatch::{exec_result_to_tool_result, tool_call_to_exec_request};
use crate::event_emitter::EventEmitter;
use crate::gate_chain::{GateChain, GateChainBuilder, GateDecision};
use crate::metering::MeteringWrapper;

/// In-process [`KernelPort`] implementation.
///
/// Construct via [`KernelEngine::builder`]; the builder takes an
/// [`EventStorePort`], a session / agent pair, at least one registered
/// backend, and three gate collaborators (policy, budget, network).
pub struct KernelEngine {
    /// Resolves `BackendSelector → Arc<dyn HypervisorBackend>`.
    registry: BackendRegistry,
    /// Composite gate consulted before dispatch / fork.
    gate_chain: Arc<GateChain>,
    /// Emits kernel-tier `EventRecord`s into the injected event store.
    emitter: Arc<EventEmitter>,
    /// Default runtime hint used when translating [`ToolCall`] into an
    /// [`aios_protocol::hypervisor::ExecRequest`].
    default_runtime: RuntimeHint,
}

impl KernelEngine {
    /// Start building an engine.
    ///
    /// All required fields surface as [`KernelEngineError::BuilderMissing`]
    /// when absent at [`KernelEngineBuilder::build`] time.
    pub fn builder() -> KernelEngineBuilder {
        KernelEngineBuilder::new()
    }

    /// Pure deterministic fold over a Lago `kernel.*` event stream.
    ///
    /// Given the events emitted during a session, reconstruct the engine's
    /// observable state: live VM handles, snapshot inventory, and
    /// per-session resource totals. This is the test oracle that proves
    /// the engine is a deterministic fold over the event journal — the
    /// foundation of the Phase 1 replay-determinism invariant (BRO-876).
    ///
    /// Accepts any iterator over `&EventKind`, so callers can replay from
    /// a `Vec<EventRecord>` (via `iter().map(|r| &r.kind)`), a Lago
    /// `read()`, or any other buffer.
    ///
    /// The fold is total: only variants that change observable state are
    /// applied; every other [`EventKind`] is counted in
    /// [`ReplayedState::events_applied`] but leaves the rest of the
    /// state untouched. Replaying the same stream twice produces
    /// byte-identical [`ReplayedState`] values (verified by the
    /// `replay_determinism` integration test).
    pub fn replay<'a, I>(events: I) -> ReplayedState
    where
        I: IntoIterator<Item = &'a EventKind>,
    {
        let mut state = ReplayedState::default();
        for kind in events {
            state.events_applied = state.events_applied.saturating_add(1);
            match kind {
                EventKind::KernelVmCreated(p) => {
                    state.live_vms.insert(
                        p.vm_id.to_string(),
                        ReplayedVm {
                            vm_id: p.vm_id.clone(),
                            backend: p.backend.clone(),
                            session_id: p.session_id.clone(),
                            agent_id: p.agent_id.clone(),
                        },
                    );
                }
                EventKind::KernelVmForked(p) => {
                    // A fork produces a child VM whose identity lives in
                    // this event. The parent's backend / session / agent
                    // is inherited when the parent is still live in the
                    // reconstructed state; otherwise the child is
                    // recorded with best-effort defaults. Real kernels
                    // emit a `KernelVmCreated` for the child in tandem
                    // with the fork (that's what populates the primary
                    // fields); this arm is a safety-net so the fork
                    // event alone still leaves the child visible.
                    let key = p.child_vm_id.to_string();
                    if !state.live_vms.contains_key(&key) {
                        let parent_key = p.parent_vm_id.to_string();
                        let (backend, session_id, agent_id) = match state.live_vms.get(&parent_key)
                        {
                            Some(parent) => (
                                parent.backend.clone(),
                                parent.session_id.clone(),
                                parent.agent_id.clone(),
                            ),
                            None => (
                                BackendId::from("unknown"),
                                SessionId::from_string(""),
                                AgentId::from_string(""),
                            ),
                        };
                        state.live_vms.insert(
                            key,
                            ReplayedVm {
                                vm_id: p.child_vm_id.clone(),
                                backend,
                                session_id,
                                agent_id,
                            },
                        );
                    }
                }
                EventKind::KernelVmSnapshotted(p) => {
                    state.snapshots.insert(
                        p.snapshot_id.to_string(),
                        ReplayedSnapshot {
                            snapshot_id: p.snapshot_id.clone(),
                            vm_id: p.vm_id.clone(),
                            name: p.name.clone(),
                            size_bytes: p.size_bytes,
                        },
                    );
                }
                EventKind::KernelVmDestroyed(p) => {
                    state.live_vms.remove(&p.vm_id.to_string());
                }
                EventKind::KernelUsageRecorded(p) => {
                    let entry = state
                        .session_usage
                        .entry(p.session_id.to_string())
                        .or_default();
                    entry.cpu_ms = entry.cpu_ms.saturating_add(p.usage.cpu_ms);
                    entry.mem_peak_kb = entry.mem_peak_kb.max(p.usage.mem_peak_kb);
                    entry.egress_bytes = entry.egress_bytes.saturating_add(p.usage.egress_bytes);
                    entry.duration_ms = entry.duration_ms.saturating_add(p.usage.duration_ms);
                    entry.syscall_count = entry.syscall_count.saturating_add(p.usage.syscall_count);
                    entry.confidence = min_confidence(entry.confidence, p.usage.confidence);
                }
                // Hibernate / Resume do not change observable live-vm
                // membership — the handle stays alive, only its status
                // flips. Dispatch events are accounted for via the
                // paired `KernelUsageRecorded`. Denied / ForkDenied are
                // audit-only and leave state untouched. Other
                // non-kernel variants are ignored.
                _ => {}
            }
        }
        state
    }
}

/// Observable state reconstructed by folding a `kernel.*` event stream.
///
/// Produced by [`KernelEngine::replay`]. Two replays of the same event
/// sequence produce byte-identical values (deterministic fold
/// invariant).
///
/// Field choice mirrors the engine's externally-visible state:
///
/// - `live_vms` — VMs that were created and not subsequently
///   [`EventKind::KernelVmDestroyed`]. `Hibernated` / `Resumed` do not
///   affect membership.
/// - `snapshots` — every snapshot ever captured in the stream. There
///   is no kernel event for snapshot deletion in Phase 1.
/// - `session_usage` — per-session cumulative [`ResourceUsage`] with
///   `cpu_ms`, `egress_bytes`, `duration_ms`, `syscall_count` summed
///   across every [`EventKind::KernelUsageRecorded`] and
///   `mem_peak_kb` taking the maximum. Confidence degrades via
///   [`min_confidence`]: a single `Unknown` downgrades the aggregate
///   to `Unknown`; any `Estimated` degrades from `Measured`.
/// - `events_applied` — total event count including ignored variants,
///   so the counter always matches the input stream's length.
///
/// Keyed by the stringified identifier to keep iteration deterministic
/// across platforms without requiring `Ord` derives on the upstream ID
/// types (which are `#[serde(transparent)]` `String` wrappers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayedState {
    /// Currently-live VMs, keyed by the stringified [`VmId`].
    pub live_vms: BTreeMap<String, ReplayedVm>,
    /// Snapshots captured during the session, keyed by the
    /// stringified [`VmSnapshotId`].
    pub snapshots: BTreeMap<String, ReplayedSnapshot>,
    /// Per-session cumulative resource usage, keyed by the
    /// stringified [`SessionId`].
    pub session_usage: BTreeMap<String, ResourceUsage>,
    /// Number of events folded so far — monotonically increasing, one
    /// per event regardless of whether the event changed observable
    /// state.
    pub events_applied: u64,
}

/// Reconstructed live-VM record held in [`ReplayedState::live_vms`].
///
/// Captures the identity of the VM plus the attribution fields carried
/// on the originating [`EventKind::KernelVmCreated`] (or inherited
/// from the parent on [`EventKind::KernelVmForked`] when the fork
/// event arrives without a matching create).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayedVm {
    /// Identifier of the VM.
    pub vm_id: VmId,
    /// Backend that hosts the VM.
    pub backend: BackendId,
    /// Session the VM was created under.
    pub session_id: SessionId,
    /// Agent the VM was created on behalf of.
    pub agent_id: AgentId,
}

/// Reconstructed snapshot record held in [`ReplayedState::snapshots`].
///
/// Captures the snapshot identity plus metadata (parent VM, human
/// name, size) as reported on [`EventKind::KernelVmSnapshotted`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayedSnapshot {
    /// Identifier of the snapshot.
    pub snapshot_id: VmSnapshotId,
    /// VM the snapshot was captured from.
    pub vm_id: VmId,
    /// Human-readable snapshot name.
    pub name: String,
    /// Size of the snapshot on disk, in bytes.
    pub size_bytes: u64,
}

/// Degrade two [`UsageConfidence`] values to the weaker of the pair.
///
/// Used when folding multiple [`EventKind::KernelUsageRecorded`]
/// events for a single session: the aggregate confidence can never
/// exceed the weakest contribution. `Unknown` is strictly weaker than
/// `Estimated`, which is strictly weaker than `Measured`.
fn min_confidence(a: UsageConfidence, b: UsageConfidence) -> UsageConfidence {
    match (a, b) {
        (UsageConfidence::Unknown, _) | (_, UsageConfidence::Unknown) => UsageConfidence::Unknown,
        (UsageConfidence::Estimated, _) | (_, UsageConfidence::Estimated) => {
            UsageConfidence::Estimated
        }
        _ => UsageConfidence::Measured,
    }
}

/// Staging builder for [`KernelEngine`].
///
/// Required collaborators: registry with at least one backend, policy
/// gate, budget gate, network isolation port, event store, session id,
/// agent id. Optional: branch id (defaults to `main`), runtime hint
/// (defaults to [`RuntimeHint::Shell`]), fork-λ gate, network policy,
/// clock.
pub struct KernelEngineBuilder {
    registry: BackendRegistry,
    registry_had_registrations: bool,
    policy_gate: Option<Arc<dyn BudgetGatePort>>,
    budget_gate: Option<Arc<dyn BudgetGatePort>>,
    fork_lambda_gate: Option<Arc<dyn BudgetGatePort>>,
    network_isolation: Option<Arc<dyn NetworkIsolationPort>>,
    network_policy: NetworkPolicy,
    event_store: Option<Arc<dyn EventStorePort>>,
    session_id: Option<SessionId>,
    agent_id: Option<AgentId>,
    branch_id: BranchId,
    runtime_hint: RuntimeHint,
    clock: Option<crate::event_emitter::Clock>,
}

impl Default for KernelEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned by [`KernelEngineBuilder::build`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KernelEngineError {
    /// A required builder field was not set.
    #[error("builder missing required field: {0}")]
    BuilderMissing(&'static str),
    /// Forwarded from the [`BackendRegistry`].
    #[error("registry error: {0}")]
    Registry(#[from] crate::backend_registry::RegistryError),
    /// Forwarded from the [`crate::gate_chain::GateChainBuilder`].
    #[error("gate chain error: {0}")]
    GateChain(#[from] crate::gate_chain::GateChainBuildError),
    /// Forwarded from the [`crate::event_emitter::EventEmitterBuilder`].
    #[error("event emitter error: {0}")]
    EventEmitter(#[from] crate::event_emitter::BuildError),
}

impl KernelEngineBuilder {
    /// Construct an empty builder.
    pub fn new() -> Self {
        Self {
            registry: BackendRegistry::new(),
            registry_had_registrations: false,
            policy_gate: None,
            budget_gate: None,
            fork_lambda_gate: None,
            network_isolation: None,
            network_policy: NetworkPolicy::default(),
            event_store: None,
            session_id: None,
            agent_id: None,
            branch_id: BranchId::main(),
            runtime_hint: RuntimeHint::Shell,
            clock: None,
        }
    }

    /// Install a pre-populated [`BackendRegistry`]. Replaces any
    /// previously held registrations.
    pub fn registry(mut self, registry: BackendRegistry) -> Self {
        self.registry = registry;
        self.registry_had_registrations = true;
        self
    }

    /// Register a backend directly on the builder's registry.
    ///
    /// Equivalent to calling [`BackendRegistry::register`] before
    /// handing the registry to [`Self::registry`], but saves the caller
    /// from building a standalone registry when they only have one
    /// backend to wire.
    pub async fn register_backend(mut self, backend: Arc<dyn HypervisorBackend>) -> Self {
        self.registry.register(backend).await;
        self.registry_had_registrations = true;
        self
    }

    /// Install the policy gate (required).
    pub fn policy_gate(mut self, gate: Arc<dyn BudgetGatePort>) -> Self {
        self.policy_gate = Some(gate);
        self
    }

    /// Install the budget gate (required).
    pub fn budget_gate(mut self, gate: Arc<dyn BudgetGatePort>) -> Self {
        self.budget_gate = Some(gate);
        self
    }

    /// Install the optional fork-λ gate.
    pub fn fork_lambda_gate(mut self, gate: Arc<dyn BudgetGatePort>) -> Self {
        self.fork_lambda_gate = Some(gate);
        self
    }

    /// Install the network isolation port (required).
    pub fn network_isolation(mut self, port: Arc<dyn NetworkIsolationPort>) -> Self {
        self.network_isolation = Some(port);
        self
    }

    /// Override the declarative network policy applied at VM start
    /// (defaults to [`NetworkPolicy::Disabled`]).
    pub fn network_policy(mut self, policy: NetworkPolicy) -> Self {
        self.network_policy = policy;
        self
    }

    /// Install the event store (required).
    pub fn event_store(mut self, store: Arc<dyn EventStorePort>) -> Self {
        self.event_store = Some(store);
        self
    }

    /// Bind the engine to a session / agent pair (required).
    pub fn session(mut self, session_id: SessionId, agent_id: AgentId) -> Self {
        self.session_id = Some(session_id);
        self.agent_id = Some(agent_id);
        self
    }

    /// Override the branch the emitter attributes events to (defaults
    /// to [`BranchId::main`]).
    pub fn branch(mut self, branch_id: BranchId) -> Self {
        self.branch_id = branch_id;
        self
    }

    /// Override the default [`RuntimeHint`] used by [`tool_call_to_exec_request`].
    pub fn runtime(mut self, hint: RuntimeHint) -> Self {
        self.runtime_hint = hint;
        self
    }

    /// Override the [`EventEmitter`]'s clock (useful for deterministic
    /// replay tests).
    pub fn clock(mut self, clock: crate::event_emitter::Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Finalise the builder.
    pub async fn build(self) -> Result<KernelEngine, KernelEngineError> {
        // Pull required fields early so we can short-circuit cleanly.
        let policy = self
            .policy_gate
            .ok_or(KernelEngineError::BuilderMissing("policy_gate"))?;
        let budget = self
            .budget_gate
            .ok_or(KernelEngineError::BuilderMissing("budget_gate"))?;
        let network = self
            .network_isolation
            .ok_or(KernelEngineError::BuilderMissing("network_isolation"))?;
        let event_store = self
            .event_store
            .ok_or(KernelEngineError::BuilderMissing("event_store"))?;
        let session_id = self
            .session_id
            .ok_or(KernelEngineError::BuilderMissing("session_id"))?;
        let agent_id = self
            .agent_id
            .ok_or(KernelEngineError::BuilderMissing("agent_id"))?;
        if !self.registry_had_registrations && self.registry.list_backend_ids().await.is_empty() {
            return Err(KernelEngineError::BuilderMissing(
                "registry (no backends registered)",
            ));
        }

        // Compose the gate chain.
        let mut gate_builder: GateChainBuilder = GateChain::builder()
            .policy(policy)
            .budget(budget)
            .network_policy(self.network_policy)
            .network_isolation(network);
        if let Some(fork_lambda) = self.fork_lambda_gate {
            gate_builder = gate_builder.fork_lambda(fork_lambda);
        }
        let gate_chain = Arc::new(gate_builder.build()?);

        // Compose the emitter.
        let mut emitter_builder = EventEmitter::builder(event_store)
            .session(session_id)
            .agent(agent_id)
            .branch(self.branch_id);
        if let Some(clock) = self.clock {
            emitter_builder = emitter_builder.clock(clock);
        }
        let emitter = Arc::new(emitter_builder.build()?);

        Ok(KernelEngine {
            registry: self.registry,
            gate_chain,
            emitter,
            default_runtime: self.runtime_hint,
        })
    }
}

#[async_trait]
impl KernelPort for KernelEngine {
    async fn create_vm(&self, spec: VmSpec, ctx: KernelContext) -> KernelResult<VmHandle> {
        let selector = spec.backend_selector.clone();
        let backend = self
            .registry
            .resolve(&selector)
            .await
            .map_err(map_registry)?;
        let vm = backend.create(spec).await?;

        // Apply network policy at VM start. A failure aborts bring-up.
        self.gate_chain.apply_network(&vm).await?;

        // Phase 1: spec_hash is a stable placeholder — the real SHA-256
        // lands in BRO-876 (replay determinism) once spec canonicalisation
        // is defined.
        self.emitter
            .emit(
                EventKind::KernelVmCreated(KernelVmCreated {
                    vm_id: vm.vm_id.clone(),
                    backend: vm.backend.clone(),
                    spec_hash: "phase1-placeholder".into(),
                    session_id: ctx.session_id.clone(),
                    agent_id: ctx.agent_id.clone(),
                }),
                None,
            )
            .await?;

        Ok(vm)
    }

    async fn dispatch(
        &self,
        vm: &VmHandle,
        call: ToolCall,
        ctx: &KernelContext,
    ) -> KernelResult<ToolResult> {
        // 1. Gate chain.
        let cost_hint = ctx.cost_hint.clone().unwrap_or_default();
        match self.gate_chain.check_dispatch(vm, ctx, &cost_hint).await {
            GateDecision::Allow => {}
            GateDecision::Deny {
                gate,
                reason,
                gate_id: _,
            } => {
                self.emitter
                    .emit(
                        EventKind::KernelDispatchDenied(KernelDispatchDenied {
                            call_id: call.call_id.clone(),
                            gate,
                            reason: reason.clone(),
                        }),
                        None,
                    )
                    .await?;
                return Err(KernelError::GateDenied { gate, reason });
            }
            GateDecision::RequireApproval { ticket: _ } => {
                // Phase 1 does not round-trip approvals end-to-end:
                // we deny dispatch so the caller sees a structured
                // error. The full Approval resolve flow is Phase 4.
                let reason = "approval required (not yet wired to resolve flow)".to_string();
                self.emitter
                    .emit(
                        EventKind::KernelDispatchDenied(KernelDispatchDenied {
                            call_id: call.call_id.clone(),
                            gate: GateKind::Policy,
                            reason: reason.clone(),
                        }),
                        None,
                    )
                    .await?;
                return Err(KernelError::GateDenied {
                    gate: GateKind::Policy,
                    reason,
                });
            }
        }

        // 2. Resolve backend + wrap with metering.
        let backend = self
            .registry
            .resolve(&BackendSelector::Explicit {
                backend: vm.backend.clone(),
            })
            .await
            .map_err(map_registry)?;
        let metering = MeteringWrapper::new(backend, Arc::clone(&self.emitter));

        // 3. Translate ToolCall → ExecRequest, dispatch, translate back.
        let req = tool_call_to_exec_request(&call, &self.default_runtime);
        let call_id = call.call_id.clone();
        let tool_name = call.tool_name.clone();
        let (exec_result, usage) = metering
            .dispatch(vm, req, ctx, call_id.clone(), tool_name.clone())
            .await?;
        Ok(exec_result_to_tool_result(
            exec_result,
            usage,
            call_id,
            tool_name,
        ))
    }

    async fn snapshot(&self, vm: &VmHandle, name: &str) -> KernelResult<VmSnapshotHandle> {
        let backend = self
            .registry
            .resolve(&BackendSelector::Explicit {
                backend: vm.backend.clone(),
            })
            .await
            .map_err(map_registry)?;
        let snapshot_id = backend.snapshot(vm).await?;

        // Phase 1: we do not yet ask the backend for the snapshot's
        // size — that arrives with the lifed persistence layer in
        // Phase 2 where snapshots get full lifecycle metadata.
        let handle = VmSnapshotHandle {
            snapshot_id: snapshot_id.clone(),
            vm_id: vm.vm_id.clone(),
            name: name.to_string(),
            created_at: Utc::now(),
            size_bytes: 0,
        };
        self.emitter
            .emit(
                EventKind::KernelVmSnapshotted(KernelVmSnapshotted {
                    vm_id: vm.vm_id.clone(),
                    snapshot_id,
                    name: name.to_string(),
                    size_bytes: 0,
                }),
                None,
            )
            .await?;
        Ok(handle)
    }

    async fn fork(
        &self,
        snapshot: &VmSnapshotHandle,
        spec: ForkSpec,
        ctx: KernelContext,
    ) -> KernelResult<VmHandle> {
        // We need a `&VmHandle` to feed the gate chain; the parent VM
        // may not be live in memory any more (snapshot-based restore
        // path). Phase 1 constructs a lightweight stub whose identity
        // bits (`vm_id`) match the snapshot; downstream gates only
        // inspect attribution + declared spec so this is adequate. A
        // richer registry lookup lands in Phase 4 alongside
        // RcsLambdaBudgetGate.
        let stub_parent = VmHandle {
            vm_id: snapshot.vm_id.clone(),
            backend: aios_protocol::hypervisor::BackendId::from("unknown"),
            session_id: ctx.session_id.clone(),
            agent_id: ctx.agent_id.clone(),
            status: aios_protocol::hypervisor::VmStatus::Snapshotted,
            created_at: snapshot.created_at,
            metadata: serde_json::Value::Null,
        };
        match self.gate_chain.check_fork(&stub_parent, &spec, &ctx).await {
            GateDecision::Allow => {}
            GateDecision::Deny {
                gate,
                reason,
                gate_id: _,
            } => {
                self.emitter
                    .emit(
                        EventKind::KernelForkDenied(KernelForkDenied {
                            parent_vm_id: snapshot.vm_id.clone(),
                            gate,
                            reason: reason.clone(),
                        }),
                        None,
                    )
                    .await?;
                return Err(KernelError::GateDenied { gate, reason });
            }
            GateDecision::RequireApproval { ticket: _ } => {
                let reason = "approval required (not yet wired to resolve flow)".to_string();
                self.emitter
                    .emit(
                        EventKind::KernelForkDenied(KernelForkDenied {
                            parent_vm_id: snapshot.vm_id.clone(),
                            gate: GateKind::Policy,
                            reason: reason.clone(),
                        }),
                        None,
                    )
                    .await?;
                return Err(KernelError::GateDenied {
                    gate: GateKind::Policy,
                    reason,
                });
            }
        }

        // Fork restore goes through the first capable backend. A more
        // precise routing lives in Phase 4 once ForkSpec carries a
        // selector override.
        let backend = self
            .registry
            .resolve(&BackendSelector::Auto)
            .await
            .map_err(map_registry)?;
        let child = backend.restore(&snapshot.snapshot_id).await?;

        self.emitter
            .emit(
                EventKind::KernelVmForked(KernelVmForked {
                    parent_vm_id: snapshot.vm_id.clone(),
                    child_vm_id: child.vm_id.clone(),
                    snapshot_id: snapshot.snapshot_id.clone(),
                }),
                None,
            )
            .await?;

        Ok(child)
    }

    async fn hibernate(&self, vm: &VmHandle) -> KernelResult<()> {
        let backend = self
            .registry
            .resolve(&BackendSelector::Explicit {
                backend: vm.backend.clone(),
            })
            .await
            .map_err(map_registry)?;
        backend.hibernate(vm).await?;
        self.emitter
            .emit(
                EventKind::KernelVmHibernated(KernelVmHibernated {
                    vm_id: vm.vm_id.clone(),
                }),
                None,
            )
            .await?;
        Ok(())
    }

    async fn resume(&self, vm: &VmHandle) -> KernelResult<VmHandle> {
        let backend = self
            .registry
            .resolve(&BackendSelector::Explicit {
                backend: vm.backend.clone(),
            })
            .await
            .map_err(map_registry)?;
        backend.resume(vm).await?;
        self.emitter
            .emit(
                EventKind::KernelVmResumed(KernelVmResumed {
                    vm_id: vm.vm_id.clone(),
                }),
                None,
            )
            .await?;
        Ok(vm.clone())
    }

    async fn destroy(&self, vm: VmHandle) -> KernelResult<()> {
        // `destroy` is idempotent per the KernelPort contract: if the
        // backend reports the VM already stopped, swallow the error and
        // still emit the Destroyed event so downstream replay sees the
        // close.
        if let Ok(backend) = self
            .registry
            .resolve(&BackendSelector::Explicit {
                backend: vm.backend.clone(),
            })
            .await
        {
            let _ = backend.destroy(&vm).await;
        }
        self.emitter
            .emit(
                EventKind::KernelVmDestroyed(KernelVmDestroyed {
                    vm_id: vm.vm_id.clone(),
                    reason: "engine_destroy".into(),
                }),
                None,
            )
            .await?;
        Ok(())
    }
}

/// Lift a [`crate::backend_registry::RegistryError`] into the richer
/// [`KernelError`] surface.
///
/// `BackendNotFound` keeps its identity; `Auto` misses and unknown
/// selector variants collapse into a capability error so downstream
/// callers see a single, stable shape.
fn map_registry(err: crate::backend_registry::RegistryError) -> KernelError {
    use crate::backend_registry::RegistryError as RE;
    match err {
        RE::BackendNotFound(id) => KernelError::BackendNotFound(id),
        RE::NoBackendMatches => KernelError::Internal("no capable backend registered".into()),
        RE::UnsupportedSelector => {
            KernelError::Internal("unsupported backend selector variant".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use aios_protocol::budget::{BudgetDecision, ResourceBudget};
    use aios_protocol::error::{
        KernelError as LegacyKernelError, KernelResult as LegacyKernelResult,
    };
    use aios_protocol::event::EventRecord;
    use aios_protocol::hypervisor::{
        BackendCapabilitySet, BackendError, BackendId, ExecRequest, ExecResult, VmId, VmSnapshotId,
        VmStatus,
    };
    use aios_protocol::ids::{AgentId, BranchId, SeqNo, SessionId};
    use aios_protocol::kernel::{ChainId, WalletAttribution};
    use aios_protocol::ports::{EventRecordStream, EventStorePort};
    use aios_protocol::tool::ToolCall;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use life_kernel_gate::budget::NoOpBudgetGate;
    use life_kernel_gate::network::NoOpNetworkIsolation;

    // ── Stubs ───────────────────────────────────────────────────────

    /// In-memory event store (mirrors the one in event_emitter.rs /
    /// metering.rs — kept local so engine tests stay hermetic).
    struct StubEventStore {
        events: Mutex<Vec<EventRecord>>,
    }

    impl StubEventStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Vec::new()),
            })
        }

        fn stored_events(&self) -> Vec<EventRecord> {
            self.events.lock().expect("poisoned mutex").clone()
        }
    }

    #[async_trait]
    impl EventStorePort for StubEventStore {
        async fn append(&self, mut event: EventRecord) -> LegacyKernelResult<EventRecord> {
            let mut buf = self.events.lock().expect("poisoned mutex");
            event.sequence = buf.len() as SeqNo;
            buf.push(event.clone());
            Ok(event)
        }

        async fn read(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
            _from_sequence: u64,
            _limit: usize,
        ) -> LegacyKernelResult<Vec<EventRecord>> {
            Ok(self.stored_events())
        }

        async fn head(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
        ) -> LegacyKernelResult<u64> {
            Ok(self.stored_events().len() as u64)
        }

        async fn subscribe(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
            _after_sequence: u64,
        ) -> LegacyKernelResult<EventRecordStream> {
            unimplemented!("subscribe not used in engine tests")
        }
    }

    /// Configurable hypervisor backend for engine tests.
    ///
    /// Returns a canned `ExecResult` (or backend error) from `exec`, a
    /// canned snapshot id from `snapshot`, and echoes `create` /
    /// `restore` via `canned_handle`. Instances are cheap and do not
    /// share state with each other.
    struct StubBackend {
        name: &'static str,
        exec_behaviour: ExecBehaviour,
        /// When `true`, `destroy` returns an error to exercise the
        /// engine's idempotent swallow.
        destroy_errors: bool,
    }

    enum ExecBehaviour {
        Ok { stdout: Vec<u8>, exit_code: i32 },
        Err(&'static str),
    }

    impl StubBackend {
        fn ok(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                exec_behaviour: ExecBehaviour::Ok {
                    stdout: b"hello".to_vec(),
                    exit_code: 0,
                },
                destroy_errors: false,
            })
        }

        fn erroring(name: &'static str, msg: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                exec_behaviour: ExecBehaviour::Err(msg),
                destroy_errors: false,
            })
        }

        fn destroy_errors(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                exec_behaviour: ExecBehaviour::Ok {
                    stdout: b"ok".to_vec(),
                    exit_code: 0,
                },
                destroy_errors: true,
            })
        }
    }

    #[async_trait]
    impl HypervisorBackend for StubBackend {
        fn name(&self) -> &'static str {
            self.name
        }

        fn capabilities(&self) -> BackendCapabilitySet {
            BackendCapabilitySet::FILESYSTEM_READ | BackendCapabilitySet::PERSISTENCE
        }

        async fn create(&self, _spec: VmSpec) -> Result<VmHandle, BackendError> {
            Ok(canned_handle(self.name, "vm-created"))
        }

        async fn exec(
            &self,
            _vm: &VmHandle,
            _req: ExecRequest,
        ) -> Result<ExecResult, BackendError> {
            match &self.exec_behaviour {
                ExecBehaviour::Ok { stdout, exit_code } => Ok(ExecResult {
                    stdout: stdout.clone(),
                    stderr: Vec::new(),
                    exit_code: *exit_code,
                    duration_ms: 1,
                }),
                ExecBehaviour::Err(msg) => Err(BackendError::Internal((*msg).into())),
            }
        }

        async fn snapshot(&self, _vm: &VmHandle) -> Result<VmSnapshotId, BackendError> {
            Ok(VmSnapshotId::from("snap-stub"))
        }

        async fn restore(&self, _snapshot: &VmSnapshotId) -> Result<VmHandle, BackendError> {
            Ok(canned_handle(self.name, "vm-forked"))
        }

        async fn destroy(&self, _vm: &VmHandle) -> Result<(), BackendError> {
            if self.destroy_errors {
                Err(BackendError::Internal("already stopped".into()))
            } else {
                Ok(())
            }
        }

        async fn hibernate(&self, _vm: &VmHandle) -> Result<(), BackendError> {
            Ok(())
        }

        async fn resume(&self, _vm: &VmHandle) -> Result<(), BackendError> {
            Ok(())
        }
    }

    /// Policy gate that always denies with a canned reason.
    struct DenyPolicyGate;

    #[async_trait]
    impl BudgetGatePort for DenyPolicyGate {
        async fn check_dispatch(
            &self,
            _ctx: &KernelContext,
            _cost_hint: &ResourceBudget,
        ) -> BudgetDecision {
            BudgetDecision::Deny {
                reason: "policy rejects".into(),
                gate_id: "policy-static".into(),
            }
        }

        async fn check_fork(
            &self,
            _parent: &VmHandle,
            _spec: &ForkSpec,
            _ctx: &KernelContext,
        ) -> BudgetDecision {
            BudgetDecision::Deny {
                reason: "policy rejects fork".into(),
                gate_id: "policy-static".into(),
            }
        }
    }

    // ── Fixtures ────────────────────────────────────────────────────

    fn canned_handle(backend_name: &str, vm_id: &str) -> VmHandle {
        VmHandle {
            vm_id: VmId::from(vm_id),
            backend: BackendId::from(backend_name),
            session_id: SessionId::from_string("sess-engine"),
            agent_id: AgentId::from_string("agent-engine"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    fn ctx() -> KernelContext {
        KernelContext {
            session_id: SessionId::from_string("sess-engine"),
            agent_id: AgentId::from_string("agent-engine"),
            wallet: WalletAttribution {
                address: "0x0".into(),
                chain: ChainId::base(),
            },
            cost_hint: None,
            trace_ctx: None,
        }
    }

    fn spec(backend_name: &str) -> VmSpec {
        VmSpec {
            backend_selector: BackendSelector::Explicit {
                backend: BackendId::from(backend_name),
            },
            resources: Default::default(),
            network_policy: NetworkPolicy::Disabled,
            mounts: Vec::new(),
            env: Default::default(),
            runtime_hint: RuntimeHint::Shell,
            labels: Default::default(),
        }
    }

    fn frozen_clock() -> crate::event_emitter::Clock {
        let fixed = Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0).unwrap();
        Arc::new(move || fixed)
    }

    async fn build_engine(
        store: Arc<StubEventStore>,
        backend: Arc<dyn HypervisorBackend>,
        policy_gate: Arc<dyn BudgetGatePort>,
    ) -> KernelEngine {
        match KernelEngine::builder()
            .policy_gate(policy_gate)
            .budget_gate(Arc::new(NoOpBudgetGate::new()))
            .network_isolation(Arc::new(NoOpNetworkIsolation::new()))
            .event_store(store)
            .session(
                SessionId::from_string("sess-engine"),
                AgentId::from_string("agent-engine"),
            )
            .clock(frozen_clock())
            .register_backend(backend)
            .await
            .build()
            .await
        {
            Ok(engine) => engine,
            Err(e) => panic!("engine build failed: {e:?}"),
        }
    }

    // ── Tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn engine_create_vm_emits_kernel_vm_created() {
        let store = StubEventStore::new();
        let engine = build_engine(
            store.clone(),
            StubBackend::ok("stub"),
            Arc::new(NoOpBudgetGate::new()),
        )
        .await;

        let vm = engine
            .create_vm(spec("stub"), ctx())
            .await
            .expect("create_vm should succeed");
        assert_eq!(vm.vm_id, VmId::from("vm-created"));

        let stored = store.stored_events();
        assert_eq!(stored.len(), 1, "events: {stored:#?}");
        match &stored[0].kind {
            EventKind::KernelVmCreated(p) => {
                assert_eq!(p.vm_id, VmId::from("vm-created"));
                assert_eq!(p.backend, BackendId::from("stub"));
                assert_eq!(p.session_id, SessionId::from_string("sess-engine"));
            }
            other => panic!("expected KernelVmCreated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn engine_dispatch_emits_started_and_completed() {
        let store = StubEventStore::new();
        let engine = build_engine(
            store.clone(),
            StubBackend::ok("stub"),
            Arc::new(NoOpBudgetGate::new()),
        )
        .await;

        let vm = engine
            .create_vm(spec("stub"), ctx())
            .await
            .expect("create_vm should succeed");

        let call = ToolCall {
            call_id: "call-1".into(),
            tool_name: "tool.greet".into(),
            input: serde_json::json!({}),
            requested_capabilities: Vec::new(),
        };
        let result = engine
            .dispatch(&vm, call, &ctx())
            .await
            .expect("dispatch should succeed");
        assert_eq!(result.call_id, "call-1");
        assert!(!result.is_error);

        // Expect: VmCreated, DispatchStarted, DispatchCompleted, UsageRecorded.
        let stored = store.stored_events();
        assert_eq!(stored.len(), 4, "events: {stored:#?}");
        assert!(matches!(stored[0].kind, EventKind::KernelVmCreated(_)));
        assert!(matches!(
            stored[1].kind,
            EventKind::KernelDispatchStarted(_)
        ));
        assert!(matches!(
            stored[2].kind,
            EventKind::KernelDispatchCompleted(_)
        ));
        assert!(matches!(stored[3].kind, EventKind::KernelUsageRecorded(_)));
    }

    #[tokio::test]
    async fn engine_dispatch_denied_by_policy_emits_kernel_dispatch_denied() {
        let store = StubEventStore::new();
        let engine = build_engine(
            store.clone(),
            StubBackend::ok("stub"),
            Arc::new(DenyPolicyGate),
        )
        .await;

        // create_vm must succeed (policy is consulted on dispatch only).
        let vm = engine
            .create_vm(spec("stub"), ctx())
            .await
            .expect("create_vm should succeed");

        let call = ToolCall {
            call_id: "call-denied".into(),
            tool_name: "tool.blocked".into(),
            input: serde_json::json!({}),
            requested_capabilities: Vec::new(),
        };
        let err = engine
            .dispatch(&vm, call, &ctx())
            .await
            .expect_err("dispatch must fail when policy denies");
        match err {
            KernelError::GateDenied { gate, reason } => {
                assert_eq!(gate, GateKind::Policy);
                assert_eq!(reason, "policy rejects");
            }
            other => panic!("expected GateDenied(Policy), got {other:?}"),
        }

        // Expect: VmCreated, then DispatchDenied. NO Started/Completed.
        let stored = store.stored_events();
        assert_eq!(stored.len(), 2, "events: {stored:#?}");
        match &stored[1].kind {
            EventKind::KernelDispatchDenied(p) => {
                assert_eq!(p.call_id, "call-denied");
                assert_eq!(p.gate, GateKind::Policy);
                assert_eq!(p.reason, "policy rejects");
            }
            other => panic!("expected KernelDispatchDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn engine_destroy_emits_kernel_vm_destroyed_and_is_idempotent() {
        let store = StubEventStore::new();
        let engine = build_engine(
            store.clone(),
            // Backend's destroy always errors — engine must swallow it.
            StubBackend::destroy_errors("stub"),
            Arc::new(NoOpBudgetGate::new()),
        )
        .await;

        let vm = engine
            .create_vm(spec("stub"), ctx())
            .await
            .expect("create_vm should succeed");

        engine
            .destroy(vm.clone())
            .await
            .expect("destroy must be idempotent");

        // Destroy should still emit the KernelVmDestroyed event even
        // when the backend complains.
        let stored = store.stored_events();
        assert_eq!(stored.len(), 2);
        match &stored[1].kind {
            EventKind::KernelVmDestroyed(p) => {
                assert_eq!(p.vm_id, VmId::from("vm-created"));
                assert_eq!(p.reason, "engine_destroy");
            }
            other => panic!("expected KernelVmDestroyed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn engine_backend_error_surfaces_as_kernel_backend_error() {
        let store = StubEventStore::new();
        let engine = build_engine(
            store.clone(),
            StubBackend::erroring("stub", "boom"),
            Arc::new(NoOpBudgetGate::new()),
        )
        .await;

        let vm = engine
            .create_vm(spec("stub"), ctx())
            .await
            .expect("create_vm should succeed");
        let call = ToolCall {
            call_id: "call-boom".into(),
            tool_name: "tool.broken".into(),
            input: serde_json::json!({}),
            requested_capabilities: Vec::new(),
        };
        let err = engine
            .dispatch(&vm, call, &ctx())
            .await
            .expect_err("backend error must surface");
        match err {
            KernelError::Backend(BackendError::Internal(msg)) => assert_eq!(msg, "boom"),
            other => panic!("expected KernelError::Backend(Internal), got {other:?}"),
        }

        // The metering wrapper still records Started+Completed so the
        // journal stays balanced — plus VmCreated before it.
        let stored = store.stored_events();
        assert_eq!(stored.len(), 3);
        assert!(matches!(
            stored[1].kind,
            EventKind::KernelDispatchStarted(_)
        ));
        match &stored[2].kind {
            EventKind::KernelDispatchCompleted(p) => assert_eq!(p.exit_code, -1),
            other => panic!("expected KernelDispatchCompleted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn engine_snapshot_emits_snapshotted_event() {
        let store = StubEventStore::new();
        let engine = build_engine(
            store.clone(),
            StubBackend::ok("stub"),
            Arc::new(NoOpBudgetGate::new()),
        )
        .await;

        let vm = engine
            .create_vm(spec("stub"), ctx())
            .await
            .expect("create_vm should succeed");
        let handle = engine
            .snapshot(&vm, "pre-fork")
            .await
            .expect("snapshot should succeed");
        assert_eq!(handle.snapshot_id, VmSnapshotId::from("snap-stub"));
        assert_eq!(handle.name, "pre-fork");

        let stored = store.stored_events();
        assert_eq!(stored.len(), 2);
        match &stored[1].kind {
            EventKind::KernelVmSnapshotted(p) => {
                assert_eq!(p.snapshot_id, VmSnapshotId::from("snap-stub"));
                assert_eq!(p.name, "pre-fork");
            }
            other => panic!("expected KernelVmSnapshotted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn engine_builder_missing_fields_returns_errors() {
        // Missing policy gate.
        match KernelEngine::builder()
            .budget_gate(Arc::new(NoOpBudgetGate::new()))
            .network_isolation(Arc::new(NoOpNetworkIsolation::new()))
            .event_store(StubEventStore::new())
            .session(
                SessionId::from_string("sess"),
                AgentId::from_string("agent"),
            )
            .register_backend(StubBackend::ok("stub"))
            .await
            .build()
            .await
        {
            Err(KernelEngineError::BuilderMissing("policy_gate")) => {}
            Err(other) => panic!("expected BuilderMissing(policy_gate), got {other:?}"),
            Ok(_) => panic!("expected BuilderMissing, got Ok"),
        }

        // Missing backend registrations.
        match KernelEngine::builder()
            .policy_gate(Arc::new(NoOpBudgetGate::new()))
            .budget_gate(Arc::new(NoOpBudgetGate::new()))
            .network_isolation(Arc::new(NoOpNetworkIsolation::new()))
            .event_store(StubEventStore::new())
            .session(
                SessionId::from_string("sess"),
                AgentId::from_string("agent"),
            )
            .build()
            .await
        {
            Err(KernelEngineError::BuilderMissing(field)) => {
                assert!(field.contains("registry"), "unexpected field: {field}");
            }
            Err(other) => panic!("expected BuilderMissing(registry), got {other:?}"),
            Ok(_) => panic!("expected BuilderMissing(registry), got Ok"),
        }
    }

    // Compile-time assertion: `KernelEngine` is a valid `KernelPort`
    // implementation — catches accidental break of trait signature at
    // build time.
    #[allow(dead_code)]
    fn _engine_is_kernel_port(engine: Arc<KernelEngine>) -> Arc<dyn KernelPort> {
        engine
    }

    // Silence the unused-import lint for `LegacyKernelError` when the
    // engine tests above don't exercise every code path; the type is
    // re-used by future tests in the file's same scope.
    #[allow(dead_code)]
    fn _assert_legacy_err(_: LegacyKernelError) {}
}
