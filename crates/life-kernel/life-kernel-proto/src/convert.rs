//! Bridges between the generated [`crate::pb`] wire types and the
//! canonical `aios_protocol` types.
//!
//! Eight bridges are covered in this commit (BRO-857 / Task 4):
//!
//! 1. [`VmHandle`](aios_protocol::hypervisor::VmHandle) ↔ [`pb::VmHandle`]
//! 2. [`VmSnapshotHandle`](aios_protocol::hypervisor::VmSnapshotHandle) ↔ [`pb::VmSnapshotHandle`]
//! 3. [`VmSpec`](aios_protocol::hypervisor::VmSpec) ↔ [`pb::VmSpec`]
//! 4. [`ForkSpec`](aios_protocol::hypervisor::ForkSpec) ↔ [`pb::ForkSpec`]
//! 5. [`ToolCall`](aios_protocol::tool::ToolCall) ↔ [`pb::ToolCall`]
//! 6. [`ToolResult`](aios_protocol::tool::ToolResult) ↔ [`pb::ToolResult`]
//! 7. [`KernelContext`](aios_protocol::kernel::KernelContext) ↔ [`pb::KernelContext`]
//! 8. [`ResourceUsage`](aios_protocol::budget::ResourceUsage) ↔ [`pb::ResourceUsage`]
//!
//! Opaque JSON bytes fields (`network_policy_json`, `metadata_json`,
//! `input_json`, `output_json`) round-trip through
//! [`serde_json::to_vec`] / [`serde_json::from_slice`]. Everything else
//! is a direct structural map.
//!
//! All conversions are fallible via [`ConvertError`] — proto wrappers
//! use `Option<T>` on every nested message, and the bridge rejects
//! malformed wires (`None` where a struct is required, an unknown
//! `RuntimeHintKind`, a missing `BackendSelector.kind`, unparseable
//! JSON, etc.).

use std::collections::HashMap;

use aios_protocol::{
    budget::{ResourceBudget, ResourceUsage, UsageConfidence},
    hypervisor::{
        BackendId, BackendSelector, ForkSpec, Mount, RuntimeHint, VmHandle, VmId, VmInfo,
        VmResources, VmSnapshotHandle, VmSnapshotId, VmSpec, VmSpecOverrides, VmStatus,
    },
    ids::{AgentId, SessionId},
    kernel::{ChainId, KernelContext, TraceContext, WalletAttribution},
    sandbox::NetworkPolicy,
    tool::{ToolCall, ToolContent, ToolResult},
};
use chrono::{DateTime, TimeZone, Utc};

use crate::pb;

/// Error returned when a [`pb`] message cannot be translated to an
/// `aios_protocol` canonical type.
///
/// Most variants map to "a required nested message was missing on the
/// wire"; [`ConvertError::Json`] flags a malformed opaque JSON bytes
/// field; [`ConvertError::UnknownEnum`] flags an enum tag the current
/// build does not recognise.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConvertError {
    /// A required nested message was absent on the wire.
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    /// An opaque JSON bytes field failed to (de)serialise.
    #[error("json serde error in field `{field}`: {source}")]
    Json {
        /// Name of the offending proto field.
        field: &'static str,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// An enum value outside the set the current build understands.
    #[error("unknown enum value for {kind}: {value}")]
    UnknownEnum {
        /// Name of the enum as declared in the proto file.
        kind: &'static str,
        /// The raw integer or string value that was rejected.
        value: String,
    },
    /// An out-of-range timestamp (nanoseconds or seconds).
    #[error("invalid timestamp: seconds={seconds}, nanos={nanos}")]
    InvalidTimestamp {
        /// Seconds component that failed to validate.
        seconds: i64,
        /// Nanoseconds component that failed to validate.
        nanos: i32,
    },
}

// ── Timestamp helpers ────────────────────────────────────────────────

fn chrono_to_proto(ts: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: ts.timestamp(),
        nanos: ts.timestamp_subsec_nanos() as i32,
    }
}

fn proto_to_chrono(ts: prost_types::Timestamp) -> Result<DateTime<Utc>, ConvertError> {
    match Utc.timestamp_opt(ts.seconds, ts.nanos as u32) {
        chrono::LocalResult::Single(dt) => Ok(dt),
        _ => Err(ConvertError::InvalidTimestamp {
            seconds: ts.seconds,
            nanos: ts.nanos,
        }),
    }
}

// ── Identity bridges (private — never user-facing errors) ────────────

impl From<VmId> for pb::VmId {
    fn from(v: VmId) -> Self {
        Self { value: v.0 }
    }
}

impl From<pb::VmId> for VmId {
    fn from(v: pb::VmId) -> Self {
        Self(v.value)
    }
}

impl From<VmSnapshotId> for pb::VmSnapshotId {
    fn from(v: VmSnapshotId) -> Self {
        Self { value: v.0 }
    }
}

impl From<pb::VmSnapshotId> for VmSnapshotId {
    fn from(v: pb::VmSnapshotId) -> Self {
        Self(v.value)
    }
}

impl From<BackendId> for pb::BackendId {
    fn from(v: BackendId) -> Self {
        Self { value: v.0 }
    }
}

impl From<pb::BackendId> for BackendId {
    fn from(v: pb::BackendId) -> Self {
        Self(v.value)
    }
}

impl From<SessionId> for pb::SessionId {
    fn from(v: SessionId) -> Self {
        Self {
            value: v.as_str().to_owned(),
        }
    }
}

impl From<pb::SessionId> for SessionId {
    fn from(v: pb::SessionId) -> Self {
        Self::from_string(v.value)
    }
}

impl From<AgentId> for pb::AgentId {
    fn from(v: AgentId) -> Self {
        Self {
            value: v.as_str().to_owned(),
        }
    }
}

impl From<pb::AgentId> for AgentId {
    fn from(v: pb::AgentId) -> Self {
        Self::from_string(v.value)
    }
}

// ── VmResources (infallible — fixed-width primitives) ────────────────

impl From<VmResources> for pb::VmResources {
    fn from(r: VmResources) -> Self {
        Self {
            vcpus: r.vcpus,
            memory_kb: r.memory_kb,
            disk_kb: r.disk_kb,
            timeout_secs: r.timeout_secs,
        }
    }
}

impl From<pb::VmResources> for VmResources {
    fn from(r: pb::VmResources) -> Self {
        Self {
            vcpus: r.vcpus,
            memory_kb: r.memory_kb,
            disk_kb: r.disk_kb,
            timeout_secs: r.timeout_secs,
        }
    }
}

// ── Mount ────────────────────────────────────────────────────────────

impl From<Mount> for pb::Mount {
    fn from(m: Mount) -> Self {
        Self {
            source: m.source,
            target: m.target,
            read_only: m.read_only,
        }
    }
}

impl From<pb::Mount> for Mount {
    fn from(m: pb::Mount) -> Self {
        Self {
            source: m.source,
            target: m.target,
            read_only: m.read_only,
        }
    }
}

// ── RuntimeHint ──────────────────────────────────────────────────────

impl TryFrom<RuntimeHint> for pb::RuntimeHint {
    type Error = ConvertError;

    fn try_from(h: RuntimeHint) -> Result<Self, Self::Error> {
        Ok(match h {
            RuntimeHint::Shell => Self {
                kind: pb::RuntimeHintKind::RuntimeHintShell as i32,
                version_or_image: String::new(),
            },
            RuntimeHint::Node { version } => Self {
                kind: pb::RuntimeHintKind::RuntimeHintNode as i32,
                version_or_image: version,
            },
            RuntimeHint::Python { version } => Self {
                kind: pb::RuntimeHintKind::RuntimeHintPython as i32,
                version_or_image: version,
            },
            RuntimeHint::Custom { image } => Self {
                kind: pb::RuntimeHintKind::RuntimeHintCustom as i32,
                version_or_image: image,
            },
            // `#[non_exhaustive]` — a future aios variant must be added
            // to the proto before it can be bridged.
            other => {
                return Err(ConvertError::UnknownEnum {
                    kind: "RuntimeHint",
                    value: format!("{other:?}"),
                });
            }
        })
    }
}

impl TryFrom<pb::RuntimeHint> for RuntimeHint {
    type Error = ConvertError;

    fn try_from(h: pb::RuntimeHint) -> Result<Self, Self::Error> {
        let kind =
            pb::RuntimeHintKind::try_from(h.kind).map_err(|_| ConvertError::UnknownEnum {
                kind: "RuntimeHintKind",
                value: h.kind.to_string(),
            })?;
        Ok(match kind {
            pb::RuntimeHintKind::RuntimeHintShell => Self::Shell,
            pb::RuntimeHintKind::RuntimeHintNode => Self::Node {
                version: h.version_or_image,
            },
            pb::RuntimeHintKind::RuntimeHintPython => Self::Python {
                version: h.version_or_image,
            },
            pb::RuntimeHintKind::RuntimeHintCustom => Self::Custom {
                image: h.version_or_image,
            },
        })
    }
}

// ── BackendSelector ──────────────────────────────────────────────────

impl TryFrom<BackendSelector> for pb::BackendSelector {
    type Error = ConvertError;

    fn try_from(s: BackendSelector) -> Result<Self, Self::Error> {
        let kind = match s {
            BackendSelector::Explicit { backend } => {
                pb::backend_selector::Kind::Explicit(backend.into())
            }
            BackendSelector::Auto => pb::backend_selector::Kind::Auto(pb::Empty {}),
            // `#[non_exhaustive]` — future variants must be added to the
            // proto before they can be bridged.
            other => {
                return Err(ConvertError::UnknownEnum {
                    kind: "BackendSelector",
                    value: format!("{other:?}"),
                });
            }
        };
        Ok(Self { kind: Some(kind) })
    }
}

impl TryFrom<pb::BackendSelector> for BackendSelector {
    type Error = ConvertError;

    fn try_from(s: pb::BackendSelector) -> Result<Self, Self::Error> {
        match s
            .kind
            .ok_or(ConvertError::MissingField("BackendSelector.kind"))?
        {
            pb::backend_selector::Kind::Explicit(id) => Ok(Self::Explicit { backend: id.into() }),
            pb::backend_selector::Kind::Auto(_) => Ok(Self::Auto),
        }
    }
}

// ── VmSpec ───────────────────────────────────────────────────────────

impl TryFrom<VmSpec> for pb::VmSpec {
    type Error = ConvertError;

    fn try_from(s: VmSpec) -> Result<Self, Self::Error> {
        let network_policy_json =
            serde_json::to_vec(&s.network_policy).map_err(|source| ConvertError::Json {
                field: "VmSpec.network_policy_json",
                source,
            })?;
        let backend_selector: pb::BackendSelector = s.backend_selector.try_into()?;
        let runtime_hint: pb::RuntimeHint = s.runtime_hint.try_into()?;
        Ok(Self {
            backend_selector: Some(backend_selector),
            resources: Some(s.resources.into()),
            network_policy_json,
            mounts: s.mounts.into_iter().map(Into::into).collect(),
            env: s.env.into_iter().collect(),
            runtime_hint: Some(runtime_hint),
            labels: s.labels.into_iter().collect(),
        })
    }
}

impl TryFrom<pb::VmSpec> for VmSpec {
    type Error = ConvertError;

    fn try_from(s: pb::VmSpec) -> Result<Self, Self::Error> {
        let backend_selector: BackendSelector = s
            .backend_selector
            .ok_or(ConvertError::MissingField("VmSpec.backend_selector"))?
            .try_into()?;
        let resources: VmResources = s
            .resources
            .ok_or(ConvertError::MissingField("VmSpec.resources"))?
            .into();
        let runtime_hint: RuntimeHint = s
            .runtime_hint
            .ok_or(ConvertError::MissingField("VmSpec.runtime_hint"))?
            .try_into()?;
        let network_policy: NetworkPolicy = serde_json::from_slice(&s.network_policy_json)
            .map_err(|source| ConvertError::Json {
                field: "VmSpec.network_policy_json",
                source,
            })?;
        let mounts = s.mounts.into_iter().map(Into::into).collect();
        let env: HashMap<String, String> = s.env.into_iter().collect();
        let labels: HashMap<String, String> = s.labels.into_iter().collect();
        Ok(Self {
            backend_selector,
            resources,
            network_policy,
            mounts,
            env,
            runtime_hint,
            labels,
        })
    }
}

// ── ForkSpec ─────────────────────────────────────────────────────────

impl From<ForkSpec> for pb::ForkSpec {
    fn from(s: ForkSpec) -> Self {
        let ForkSpec {
            parent_snapshot,
            overrides,
        } = s;
        let VmSpecOverrides {
            resources,
            env,
            labels,
        } = overrides;
        Self {
            parent_snapshot: Some(parent_snapshot.into()),
            resources_override: resources.map(Into::into),
            env_override: env.into_iter().collect(),
            label_override: labels.into_iter().collect(),
        }
    }
}

impl TryFrom<pb::ForkSpec> for ForkSpec {
    type Error = ConvertError;

    fn try_from(s: pb::ForkSpec) -> Result<Self, Self::Error> {
        let parent_snapshot: VmSnapshotId = s
            .parent_snapshot
            .ok_or(ConvertError::MissingField("ForkSpec.parent_snapshot"))?
            .into();
        let overrides = VmSpecOverrides {
            resources: s.resources_override.map(Into::into),
            env: s.env_override.into_iter().collect(),
            labels: s.label_override.into_iter().collect(),
        };
        Ok(Self {
            parent_snapshot,
            overrides,
        })
    }
}

// ── VmStatus ─────────────────────────────────────────────────────────

fn status_to_proto(s: &VmStatus) -> Result<pb::VmStatus, ConvertError> {
    Ok(match s {
        VmStatus::Starting => pb::VmStatus {
            state: "starting".into(),
            reason: String::new(),
        },
        VmStatus::Running => pb::VmStatus {
            state: "running".into(),
            reason: String::new(),
        },
        VmStatus::Hibernated => pb::VmStatus {
            state: "hibernated".into(),
            reason: String::new(),
        },
        VmStatus::Snapshotted => pb::VmStatus {
            state: "snapshotted".into(),
            reason: String::new(),
        },
        VmStatus::Stopping => pb::VmStatus {
            state: "stopping".into(),
            reason: String::new(),
        },
        VmStatus::Stopped => pb::VmStatus {
            state: "stopped".into(),
            reason: String::new(),
        },
        VmStatus::Failed { reason } => pb::VmStatus {
            state: "failed".into(),
            reason: reason.clone(),
        },
        // `#[non_exhaustive]` — new aios variants need a proto mapping.
        other => {
            return Err(ConvertError::UnknownEnum {
                kind: "VmStatus",
                value: format!("{other:?}"),
            });
        }
    })
}

fn status_from_proto(s: pb::VmStatus) -> Result<VmStatus, ConvertError> {
    Ok(match s.state.as_str() {
        "starting" => VmStatus::Starting,
        "running" => VmStatus::Running,
        "hibernated" => VmStatus::Hibernated,
        "snapshotted" => VmStatus::Snapshotted,
        "stopping" => VmStatus::Stopping,
        "stopped" => VmStatus::Stopped,
        "failed" => VmStatus::Failed { reason: s.reason },
        other => {
            return Err(ConvertError::UnknownEnum {
                kind: "VmStatus.state",
                value: other.to_owned(),
            });
        }
    })
}

// ── VmHandle ─────────────────────────────────────────────────────────

impl TryFrom<VmHandle> for pb::VmHandle {
    type Error = ConvertError;

    fn try_from(h: VmHandle) -> Result<Self, Self::Error> {
        let metadata_json =
            serde_json::to_vec(&h.metadata).map_err(|source| ConvertError::Json {
                field: "VmHandle.metadata_json",
                source,
            })?;
        let status = status_to_proto(&h.status)?;
        Ok(Self {
            vm_id: Some(h.vm_id.into()),
            backend: Some(h.backend.into()),
            session_id: Some(h.session_id.into()),
            agent_id: Some(h.agent_id.into()),
            status: Some(status),
            created_at: Some(chrono_to_proto(h.created_at)),
            metadata_json,
        })
    }
}

impl TryFrom<pb::VmHandle> for VmHandle {
    type Error = ConvertError;

    fn try_from(h: pb::VmHandle) -> Result<Self, Self::Error> {
        let vm_id: VmId = h
            .vm_id
            .ok_or(ConvertError::MissingField("VmHandle.vm_id"))?
            .into();
        let backend: BackendId = h
            .backend
            .ok_or(ConvertError::MissingField("VmHandle.backend"))?
            .into();
        let session_id: SessionId = h
            .session_id
            .ok_or(ConvertError::MissingField("VmHandle.session_id"))?
            .into();
        let agent_id: AgentId = h
            .agent_id
            .ok_or(ConvertError::MissingField("VmHandle.agent_id"))?
            .into();
        let status = status_from_proto(
            h.status
                .ok_or(ConvertError::MissingField("VmHandle.status"))?,
        )?;
        let created_at = proto_to_chrono(
            h.created_at
                .ok_or(ConvertError::MissingField("VmHandle.created_at"))?,
        )?;
        let metadata: serde_json::Value = if h.metadata_json.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&h.metadata_json).map_err(|source| ConvertError::Json {
                field: "VmHandle.metadata_json",
                source,
            })?
        };
        Ok(Self {
            vm_id,
            backend,
            session_id,
            agent_id,
            status,
            created_at,
            metadata,
        })
    }
}

// ── VmSnapshotHandle ─────────────────────────────────────────────────

impl From<VmSnapshotHandle> for pb::VmSnapshotHandle {
    fn from(h: VmSnapshotHandle) -> Self {
        Self {
            snapshot_id: Some(h.snapshot_id.into()),
            vm_id: Some(h.vm_id.into()),
            name: h.name,
            created_at: Some(chrono_to_proto(h.created_at)),
            size_bytes: h.size_bytes,
        }
    }
}

impl TryFrom<pb::VmSnapshotHandle> for VmSnapshotHandle {
    type Error = ConvertError;

    fn try_from(h: pb::VmSnapshotHandle) -> Result<Self, Self::Error> {
        let snapshot_id: VmSnapshotId = h
            .snapshot_id
            .ok_or(ConvertError::MissingField("VmSnapshotHandle.snapshot_id"))?
            .into();
        let vm_id: VmId = h
            .vm_id
            .ok_or(ConvertError::MissingField("VmSnapshotHandle.vm_id"))?
            .into();
        let created_at = proto_to_chrono(
            h.created_at
                .ok_or(ConvertError::MissingField("VmSnapshotHandle.created_at"))?,
        )?;
        Ok(Self {
            snapshot_id,
            vm_id,
            name: h.name,
            created_at,
            size_bytes: h.size_bytes,
        })
    }
}

// ── VmInfo (convenience; not in the eight, but trivial) ──────────────

impl TryFrom<VmInfo> for pb::VmInfo {
    type Error = ConvertError;

    fn try_from(i: VmInfo) -> Result<Self, Self::Error> {
        let status = status_to_proto(&i.status)?;
        Ok(Self {
            vm_id: Some(i.vm_id.into()),
            backend: Some(i.backend.into()),
            status: Some(status),
            created_at: Some(chrono_to_proto(i.created_at)),
        })
    }
}

impl TryFrom<pb::VmInfo> for VmInfo {
    type Error = ConvertError;

    fn try_from(i: pb::VmInfo) -> Result<Self, Self::Error> {
        let vm_id: VmId = i
            .vm_id
            .ok_or(ConvertError::MissingField("VmInfo.vm_id"))?
            .into();
        let backend: BackendId = i
            .backend
            .ok_or(ConvertError::MissingField("VmInfo.backend"))?
            .into();
        let status = status_from_proto(
            i.status
                .ok_or(ConvertError::MissingField("VmInfo.status"))?,
        )?;
        let created_at = proto_to_chrono(
            i.created_at
                .ok_or(ConvertError::MissingField("VmInfo.created_at"))?,
        )?;
        Ok(Self {
            vm_id,
            backend,
            status,
            created_at,
        })
    }
}

// ── WalletAttribution / TraceContext / ResourceBudget ────────────────

impl From<WalletAttribution> for pb::WalletAttribution {
    fn from(w: WalletAttribution) -> Self {
        Self {
            address: w.address,
            chain_caip2: w.chain.as_str().to_owned(),
        }
    }
}

impl From<pb::WalletAttribution> for WalletAttribution {
    fn from(w: pb::WalletAttribution) -> Self {
        Self {
            address: w.address,
            chain: ChainId::from_caip2(w.chain_caip2),
        }
    }
}

impl From<TraceContext> for pb::TraceContext {
    fn from(t: TraceContext) -> Self {
        Self {
            traceparent: t.traceparent,
            tracestate: t.tracestate,
        }
    }
}

impl From<pb::TraceContext> for TraceContext {
    fn from(t: pb::TraceContext) -> Self {
        Self {
            traceparent: t.traceparent,
            tracestate: t.tracestate,
        }
    }
}

impl From<ResourceBudget> for pb::ResourceBudget {
    fn from(b: ResourceBudget) -> Self {
        Self {
            max_cpu_ms: b.max_cpu_ms,
            max_mem_kb: b.max_mem_kb,
            max_egress_bytes: b.max_egress_bytes,
            max_duration_ms: b.max_duration_ms,
            max_syscalls: b.max_syscalls,
        }
    }
}

impl From<pb::ResourceBudget> for ResourceBudget {
    fn from(b: pb::ResourceBudget) -> Self {
        Self {
            max_cpu_ms: b.max_cpu_ms,
            max_mem_kb: b.max_mem_kb,
            max_egress_bytes: b.max_egress_bytes,
            max_duration_ms: b.max_duration_ms,
            max_syscalls: b.max_syscalls,
        }
    }
}

// ── KernelContext ────────────────────────────────────────────────────

impl From<KernelContext> for pb::KernelContext {
    fn from(c: KernelContext) -> Self {
        Self {
            session_id: Some(c.session_id.into()),
            agent_id: Some(c.agent_id.into()),
            wallet: Some(c.wallet.into()),
            cost_hint: c.cost_hint.map(Into::into),
            trace_ctx: c.trace_ctx.map(Into::into),
        }
    }
}

impl TryFrom<pb::KernelContext> for KernelContext {
    type Error = ConvertError;

    fn try_from(c: pb::KernelContext) -> Result<Self, Self::Error> {
        let session_id: SessionId = c
            .session_id
            .ok_or(ConvertError::MissingField("KernelContext.session_id"))?
            .into();
        let agent_id: AgentId = c
            .agent_id
            .ok_or(ConvertError::MissingField("KernelContext.agent_id"))?
            .into();
        let wallet: WalletAttribution = c
            .wallet
            .ok_or(ConvertError::MissingField("KernelContext.wallet"))?
            .into();
        Ok(Self {
            session_id,
            agent_id,
            wallet,
            cost_hint: c.cost_hint.map(Into::into),
            trace_ctx: c.trace_ctx.map(Into::into),
        })
    }
}

// ── ToolCall ─────────────────────────────────────────────────────────

impl TryFrom<ToolCall> for pb::ToolCall {
    type Error = ConvertError;

    fn try_from(c: ToolCall) -> Result<Self, Self::Error> {
        let input_json = serde_json::to_vec(&c.input).map_err(|source| ConvertError::Json {
            field: "ToolCall.input_json",
            source,
        })?;
        Ok(Self {
            call_id: c.call_id,
            tool_name: c.tool_name,
            input_json,
            requested_capabilities: c
                .requested_capabilities
                .into_iter()
                .map(|cap| cap.as_str().to_owned())
                .collect(),
        })
    }
}

impl TryFrom<pb::ToolCall> for ToolCall {
    type Error = ConvertError;

    fn try_from(c: pb::ToolCall) -> Result<Self, Self::Error> {
        let input: serde_json::Value = if c.input_json.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&c.input_json).map_err(|source| ConvertError::Json {
                field: "ToolCall.input_json",
                source,
            })?
        };
        Ok(Self {
            call_id: c.call_id,
            tool_name: c.tool_name,
            input,
            requested_capabilities: c
                .requested_capabilities
                .into_iter()
                .map(aios_protocol::policy::Capability::new)
                .collect(),
        })
    }
}

// ── ToolContent ──────────────────────────────────────────────────────

impl TryFrom<ToolContent> for pb::ToolContent {
    type Error = ConvertError;

    fn try_from(c: ToolContent) -> Result<Self, Self::Error> {
        let content = match c {
            ToolContent::Text { text } => pb::tool_content::Content::Text(text),
            ToolContent::Image { data, mime_type } => {
                pb::tool_content::Content::Image(pb::ImagePayload { data, mime_type })
            }
            ToolContent::Json { value } => {
                let bytes = serde_json::to_vec(&value).map_err(|source| ConvertError::Json {
                    field: "ToolContent.Json.json_value",
                    source,
                })?;
                pb::tool_content::Content::JsonValue(bytes)
            }
        };
        Ok(Self {
            content: Some(content),
        })
    }
}

impl TryFrom<pb::ToolContent> for ToolContent {
    type Error = ConvertError;

    fn try_from(c: pb::ToolContent) -> Result<Self, Self::Error> {
        match c
            .content
            .ok_or(ConvertError::MissingField("ToolContent.content"))?
        {
            pb::tool_content::Content::Text(text) => Ok(Self::Text { text }),
            pb::tool_content::Content::Image(img) => Ok(Self::Image {
                data: img.data,
                mime_type: img.mime_type,
            }),
            pb::tool_content::Content::JsonValue(bytes) => {
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|source| ConvertError::Json {
                        field: "ToolContent.Json.json_value",
                        source,
                    })?;
                Ok(Self::Json { value })
            }
        }
    }
}

// ── ResourceUsage ────────────────────────────────────────────────────

fn confidence_to_str(c: UsageConfidence) -> Result<&'static str, ConvertError> {
    Ok(match c {
        UsageConfidence::Measured => "measured",
        UsageConfidence::Estimated => "estimated",
        UsageConfidence::Unknown => "unknown",
        // `#[non_exhaustive]` — new aios variants need a string mapping.
        other => {
            return Err(ConvertError::UnknownEnum {
                kind: "UsageConfidence",
                value: format!("{other:?}"),
            });
        }
    })
}

fn confidence_from_str(s: &str) -> Result<UsageConfidence, ConvertError> {
    Ok(match s {
        "measured" => UsageConfidence::Measured,
        "estimated" => UsageConfidence::Estimated,
        "unknown" => UsageConfidence::Unknown,
        other => {
            return Err(ConvertError::UnknownEnum {
                kind: "ResourceUsage.confidence",
                value: other.to_owned(),
            });
        }
    })
}

impl TryFrom<ResourceUsage> for pb::ResourceUsage {
    type Error = ConvertError;

    fn try_from(u: ResourceUsage) -> Result<Self, Self::Error> {
        Ok(Self {
            cpu_ms: u.cpu_ms,
            mem_peak_kb: u.mem_peak_kb,
            egress_bytes: u.egress_bytes,
            duration_ms: u.duration_ms,
            syscall_count: u.syscall_count,
            confidence: confidence_to_str(u.confidence)?.to_owned(),
        })
    }
}

impl TryFrom<pb::ResourceUsage> for ResourceUsage {
    type Error = ConvertError;

    fn try_from(u: pb::ResourceUsage) -> Result<Self, Self::Error> {
        Ok(Self {
            cpu_ms: u.cpu_ms,
            mem_peak_kb: u.mem_peak_kb,
            egress_bytes: u.egress_bytes,
            duration_ms: u.duration_ms,
            syscall_count: u.syscall_count,
            confidence: confidence_from_str(&u.confidence)?,
        })
    }
}

// ── ToolResult ───────────────────────────────────────────────────────

impl TryFrom<ToolResult> for pb::ToolResult {
    type Error = ConvertError;

    fn try_from(r: ToolResult) -> Result<Self, Self::Error> {
        let output_json = serde_json::to_vec(&r.output).map_err(|source| ConvertError::Json {
            field: "ToolResult.output_json",
            source,
        })?;
        let content = r
            .content
            .unwrap_or_default()
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        let usage = r.usage.map(TryInto::try_into).transpose()?;
        Ok(Self {
            call_id: r.call_id,
            tool_name: r.tool_name,
            output_json,
            content,
            is_error: r.is_error,
            usage,
        })
    }
}

impl TryFrom<pb::ToolResult> for ToolResult {
    type Error = ConvertError;

    fn try_from(r: pb::ToolResult) -> Result<Self, Self::Error> {
        let output: serde_json::Value = if r.output_json.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&r.output_json).map_err(|source| ConvertError::Json {
                field: "ToolResult.output_json",
                source,
            })?
        };
        let content = if r.content.is_empty() {
            None
        } else {
            let vec = r
                .content
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?;
            Some(vec)
        };
        let usage = r.usage.map(TryInto::try_into).transpose()?;
        Ok(Self {
            call_id: r.call_id,
            tool_name: r.tool_name,
            output,
            content,
            is_error: r.is_error,
            usage,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────
//                              Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::hypervisor::VmStatus;
    use aios_protocol::policy::Capability;
    use chrono::TimeZone;

    fn fixed_utc() -> DateTime<Utc> {
        // Second-precision so chrono ↔ prost_types::Timestamp round-trips
        // without truncation; the proto timestamp carries nanos but
        // `DateTime<Utc>` comparison across serde cycles in aios types
        // uses JSON which is microsecond-precision. Tests below assert
        // equality on the round-tripped chrono value directly, which
        // preserves nano precision — but picking a whole second keeps
        // the fixture portable.
        Utc.with_ymd_and_hms(2026, 4, 23, 12, 34, 56).unwrap()
    }

    // ── 1. VmHandle ──

    #[test]
    fn round_trip_vm_handle() {
        let handle = VmHandle {
            vm_id: VmId::from("vm-1"),
            backend: BackendId::from("local"),
            session_id: SessionId::from_string("sess-1"),
            agent_id: AgentId::from_string("agent-1"),
            status: VmStatus::Running,
            created_at: fixed_utc(),
            metadata: serde_json::json!({ "region": "us-east-1", "tier": 2 }),
        };
        let wire: pb::VmHandle = handle.clone().try_into().unwrap();
        let back: VmHandle = wire.try_into().unwrap();
        assert_eq!(back.vm_id, handle.vm_id);
        assert_eq!(back.backend, handle.backend);
        assert_eq!(back.session_id, handle.session_id);
        assert_eq!(back.agent_id, handle.agent_id);
        assert_eq!(back.status, handle.status);
        assert_eq!(back.created_at, handle.created_at);
        assert_eq!(back.metadata, handle.metadata);
    }

    // ── 2. VmSnapshotHandle ──

    #[test]
    fn round_trip_vm_snapshot_handle() {
        let snap = VmSnapshotHandle {
            snapshot_id: VmSnapshotId::from("snap-1"),
            vm_id: VmId::from("vm-1"),
            name: "pre-fork".into(),
            created_at: fixed_utc(),
            size_bytes: 4096,
        };
        let wire: pb::VmSnapshotHandle = snap.clone().into();
        let back: VmSnapshotHandle = wire.try_into().unwrap();
        assert_eq!(back.snapshot_id, snap.snapshot_id);
        assert_eq!(back.vm_id, snap.vm_id);
        assert_eq!(back.name, snap.name);
        assert_eq!(back.created_at, snap.created_at);
        assert_eq!(back.size_bytes, snap.size_bytes);
    }

    // ── 3. VmSpec ──

    #[test]
    fn round_trip_vm_spec() {
        let mut env = HashMap::new();
        env.insert("PATH".to_owned(), "/usr/bin".to_owned());
        let mut labels = HashMap::new();
        labels.insert("tier".to_owned(), "prod".to_owned());
        let spec = VmSpec {
            backend_selector: BackendSelector::Explicit {
                backend: BackendId::from("local"),
            },
            resources: VmResources::default(),
            network_policy: NetworkPolicy::AllowList {
                hosts: vec!["example.com".into()],
            },
            mounts: vec![Mount {
                source: "/host".into(),
                target: "/guest".into(),
                read_only: true,
            }],
            env: env.clone(),
            runtime_hint: RuntimeHint::Python {
                version: "3.12".into(),
            },
            labels: labels.clone(),
        };
        let wire: pb::VmSpec = spec.clone().try_into().unwrap();
        let back: VmSpec = wire.try_into().unwrap();
        assert_eq!(back.backend_selector, spec.backend_selector);
        assert_eq!(back.resources, spec.resources);
        assert_eq!(back.network_policy, spec.network_policy);
        assert_eq!(back.mounts, spec.mounts);
        assert_eq!(back.env, spec.env);
        assert_eq!(back.runtime_hint, spec.runtime_hint);
        assert_eq!(back.labels, spec.labels);
    }

    // ── 4. ForkSpec ──

    #[test]
    fn round_trip_fork_spec() {
        let mut env = HashMap::new();
        env.insert("TOKEN".to_owned(), "secret".to_owned());
        let overrides = VmSpecOverrides {
            resources: Some(VmResources {
                vcpus: 4,
                memory_kb: 1024 * 1024,
                disk_kb: 2_097_152,
                timeout_secs: 120,
            }),
            env: env.clone(),
            labels: HashMap::new(),
        };
        let spec = ForkSpec {
            parent_snapshot: VmSnapshotId::from("snap-1"),
            overrides,
        };
        let wire: pb::ForkSpec = spec.clone().into();
        let back: ForkSpec = wire.try_into().unwrap();
        assert_eq!(back.parent_snapshot, spec.parent_snapshot);
        assert_eq!(back.overrides.resources, spec.overrides.resources);
        assert_eq!(back.overrides.env, spec.overrides.env);
        assert_eq!(back.overrides.labels, spec.overrides.labels);
    }

    // ── 5. ToolCall ──

    #[test]
    fn round_trip_tool_call() {
        let call = ToolCall {
            call_id: "call-1".into(),
            tool_name: "echo".into(),
            input: serde_json::json!({ "value": "hello" }),
            requested_capabilities: vec![Capability::new("exec:cmd:echo")],
        };
        let wire: pb::ToolCall = call.clone().try_into().unwrap();
        let back: ToolCall = wire.try_into().unwrap();
        assert_eq!(back.call_id, call.call_id);
        assert_eq!(back.tool_name, call.tool_name);
        assert_eq!(back.input, call.input);
        assert_eq!(back.requested_capabilities, call.requested_capabilities);
    }

    // ── 6. ToolResult ──

    #[test]
    fn round_trip_tool_result() {
        let result = ToolResult {
            call_id: "call-1".into(),
            tool_name: "echo".into(),
            output: serde_json::json!({ "ok": true, "value": "hello" }),
            content: Some(vec![
                ToolContent::Text {
                    text: "hello".into(),
                },
                ToolContent::Json {
                    value: serde_json::json!({ "data": 42 }),
                },
            ]),
            is_error: false,
            usage: Some(ResourceUsage {
                cpu_ms: 12,
                mem_peak_kb: 4096,
                egress_bytes: 0,
                duration_ms: 18,
                syscall_count: 7,
                confidence: UsageConfidence::Estimated,
            }),
        };
        let wire: pb::ToolResult = result.clone().try_into().unwrap();
        let back: ToolResult = wire.try_into().unwrap();
        assert_eq!(back.call_id, result.call_id);
        assert_eq!(back.tool_name, result.tool_name);
        assert_eq!(back.output, result.output);
        assert_eq!(back.content, result.content);
        assert_eq!(back.is_error, result.is_error);
        assert_eq!(back.usage, result.usage);
    }

    // ── 7. KernelContext ──

    #[test]
    fn round_trip_kernel_context() {
        let ctx = KernelContext {
            session_id: SessionId::from_string("sess-1"),
            agent_id: AgentId::from_string("agent-1"),
            wallet: WalletAttribution {
                address: "0xabcdef".into(),
                chain: ChainId::base(),
            },
            cost_hint: Some(ResourceBudget {
                max_cpu_ms: Some(1_000),
                max_duration_ms: Some(5_000),
                ..Default::default()
            }),
            trace_ctx: Some(TraceContext {
                traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into(),
                tracestate: Some("rojo=00f067aa0ba902b7".into()),
            }),
        };
        let wire: pb::KernelContext = ctx.clone().into();
        let back: KernelContext = wire.try_into().unwrap();
        assert_eq!(back.session_id, ctx.session_id);
        assert_eq!(back.agent_id, ctx.agent_id);
        assert_eq!(back.wallet, ctx.wallet);
        assert_eq!(back.cost_hint, ctx.cost_hint);
        assert_eq!(back.trace_ctx, ctx.trace_ctx);
    }

    // ── 8. ResourceUsage ──

    #[test]
    fn round_trip_resource_usage_all_confidences() {
        for confidence in [
            UsageConfidence::Measured,
            UsageConfidence::Estimated,
            UsageConfidence::Unknown,
        ] {
            let usage = ResourceUsage {
                cpu_ms: 100,
                mem_peak_kb: 2048,
                egress_bytes: 512,
                duration_ms: 120,
                syscall_count: 42,
                confidence,
            };
            let wire: pb::ResourceUsage = usage.clone().try_into().unwrap();
            let back: ResourceUsage = wire.try_into().unwrap();
            assert_eq!(back, usage);
        }
    }

    // ── Negative coverage ──

    #[test]
    fn convert_error_on_unknown_confidence() {
        let wire = pb::ResourceUsage {
            cpu_ms: 0,
            mem_peak_kb: 0,
            egress_bytes: 0,
            duration_ms: 0,
            syscall_count: 0,
            confidence: "somethig-else".into(),
        };
        let err = ResourceUsage::try_from(wire).unwrap_err();
        assert!(matches!(
            err,
            ConvertError::UnknownEnum {
                kind: "ResourceUsage.confidence",
                ..
            }
        ));
    }

    #[test]
    fn convert_error_on_missing_selector_kind() {
        let wire = pb::BackendSelector { kind: None };
        let err = BackendSelector::try_from(wire).unwrap_err();
        assert!(matches!(
            err,
            ConvertError::MissingField("BackendSelector.kind")
        ));
    }
}
