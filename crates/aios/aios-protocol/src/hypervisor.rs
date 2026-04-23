//! Low-level hypervisor substrate types — shared vocabulary for VM-backed
//! execution across the Agent OS.
//!
//! These types are consumed by the (future) `HypervisorBackend` trait
//! (lands in BRO-848), implemented by backend adapter crates
//! (`arcan-provider-local`, `arcan-provider-vercel`, `arcan-provider-cube`,
//! …), and surfaced to callers through the (future) `KernelPort` trait.
//!
//! BRO-847 seeds the type vocabulary only — the traits, `BackendError`, and
//! capability flags arrive in BRO-848.

use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SessionId};

// ── Identity ─────────────────────────────────────────────────────────────────

/// Opaque, globally unique identifier for a VM instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VmId(pub String);

impl fmt::Display for VmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for VmId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for VmId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Opaque identifier for a VM filesystem/memory snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VmSnapshotId(pub String);

impl fmt::Display for VmSnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for VmSnapshotId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for VmSnapshotId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Identifier for a registered hypervisor backend (e.g. `"local"`, `"cube"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackendId(pub String);

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for BackendId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for BackendId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

// ── Resources ────────────────────────────────────────────────────────────────

/// Compute resource request for a new VM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmResources {
    pub vcpus: u32,
    pub memory_kb: u64,
    pub disk_kb: u64,
    pub timeout_secs: u64,
}

impl Default for VmResources {
    fn default() -> Self {
        Self {
            vcpus: 1,
            memory_kb: 512 * 1024,
            disk_kb: 2048 * 1024,
            timeout_secs: 60,
        }
    }
}

// ── Runtime hints ────────────────────────────────────────────────────────────

/// Hint to the backend about what runtime the guest expects.
///
/// The backend may still reject or substitute; this is a best-effort signal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeHint {
    #[default]
    Shell,
    Node {
        version: String,
    },
    Python {
        version: String,
    },
    Custom {
        image: String,
    },
}

// ── Mount ────────────────────────────────────────────────────────────────────

/// A mount of a host path / blob into the guest filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

// ── Network policy (reused from sandbox.rs) ──────────────────────────────────
//
// `VmSpec.network_policy` intentionally reuses [`crate::sandbox::NetworkPolicy`]
// so the declarative policy vocabulary stays in one place; enforcement lands
// with `NetworkIsolationPort` (BRO-849).

// ── Backend selector ─────────────────────────────────────────────────────────

/// How the kernel picks which backend runs a VM.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackendSelector {
    /// Use a specific backend by name.
    Explicit { backend: BackendId },
    /// Let the kernel pick from available backends based on capability match.
    #[default]
    Auto,
}

// ── Spec ─────────────────────────────────────────────────────────────────────

/// Full specification used to create a new VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSpec {
    #[serde(default)]
    pub backend_selector: BackendSelector,
    #[serde(default)]
    pub resources: VmResources,
    #[serde(default)]
    pub network_policy: crate::sandbox::NetworkPolicy,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub runtime_hint: RuntimeHint,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Overrides applied to a VM spec during a fork.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VmSpecOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<VmResources>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

// ── Handle & status ──────────────────────────────────────────────────────────

/// Current lifecycle state of a VM instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum VmStatus {
    Starting,
    Running,
    Hibernated,
    Snapshotted,
    Stopping,
    Stopped,
    Failed { reason: String },
}

/// Live reference to a VM returned by `create()` / `resume()` / `fork()`.
///
/// `metadata` is an opaque JSON bag so backends can stash provider-specific
/// fields without extending the ABI; callers should treat unknown keys as
/// forward-compatible. `PartialEq`/`Eq` are intentionally omitted because
/// `serde_json::Value` does not implement `Eq`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmHandle {
    pub vm_id: VmId,
    pub backend: BackendId,
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub status: VmStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Handle for a named VM snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSnapshotHandle {
    pub snapshot_id: VmSnapshotId,
    pub vm_id: VmId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub size_bytes: u64,
}

/// Request to fork a VM from a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkSpec {
    pub parent_snapshot: VmSnapshotId,
    #[serde(default)]
    pub overrides: VmSpecOverrides,
}

/// Lightweight summary for listing VMs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub vm_id: VmId,
    pub backend: BackendId,
    pub status: VmStatus,
    pub created_at: DateTime<Utc>,
}

// ── Exec (lower-level than KernelPort::dispatch) ─────────────────────────────

/// Shell-level command to execute inside a running VM.
///
/// This is the contract `HypervisorBackend` exposes (BRO-848). Higher-level
/// Tool-ABI dispatch (via `KernelPort`) translates `ToolCall` into
/// [`ExecRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRequest {
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<Vec<u8>>,
}

impl ExecRequest {
    /// Helper to build a POSIX shell invocation.
    pub fn shell(command: impl Into<String>) -> Self {
        Self {
            command: vec!["/bin/sh".into(), "-c".into(), command.into()],
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: None,
            stdin: None,
        }
    }
}

/// Result of an [`ExecRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    pub duration_ms: u64,
}

/// A single file to write into a VM filesystem via the (future)
/// `HypervisorFilesystemExt` trait (BRO-848).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Identity ──

    #[test]
    fn vm_id_display_and_from() {
        assert_eq!(VmId::from("abc").to_string(), "abc");
        assert_eq!(VmId::from(String::from("xyz")).to_string(), "xyz");
    }

    #[test]
    fn vm_id_is_transparent() {
        let id = VmId::from("vm-42");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"vm-42\"");
        let back: VmId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn backend_id_from_str_trait() {
        let id: BackendId = "local".into();
        assert_eq!(id.to_string(), "local");
    }

    // ── Resources / hints / selector ──

    #[test]
    fn vm_resources_defaults() {
        let r = VmResources::default();
        assert_eq!(r.vcpus, 1);
        assert_eq!(r.memory_kb, 524_288);
        assert_eq!(r.disk_kb, 2_097_152);
        assert_eq!(r.timeout_secs, 60);
    }

    #[test]
    fn runtime_hint_default_is_shell() {
        assert_eq!(RuntimeHint::default(), RuntimeHint::Shell);
    }

    #[test]
    fn runtime_hint_node_serde() {
        let h = RuntimeHint::Node {
            version: "20.11".into(),
        };
        let json = serde_json::to_string(&h).unwrap();
        let back: RuntimeHint = serde_json::from_str(&json).unwrap();
        assert_eq!(h, back);
    }

    #[test]
    fn backend_selector_defaults_to_auto() {
        assert_eq!(BackendSelector::default(), BackendSelector::Auto);
    }

    #[test]
    fn backend_selector_explicit_serde() {
        let s = BackendSelector::Explicit {
            backend: BackendId::from("local"),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: BackendSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    // ── Status ──

    #[test]
    fn vm_status_serde_roundtrip() {
        for s in [
            VmStatus::Starting,
            VmStatus::Running,
            VmStatus::Hibernated,
            VmStatus::Snapshotted,
            VmStatus::Stopping,
            VmStatus::Stopped,
            VmStatus::Failed {
                reason: "oom".into(),
            },
        ] {
            let json = serde_json::to_string(&s).unwrap();
            let back: VmStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }

    // ── Spec ──

    #[test]
    fn vm_spec_default_network_is_disabled() {
        use crate::sandbox::NetworkPolicy;
        let spec = VmSpec {
            backend_selector: BackendSelector::Auto,
            resources: VmResources::default(),
            network_policy: NetworkPolicy::default(),
            mounts: Vec::new(),
            env: HashMap::new(),
            runtime_hint: RuntimeHint::default(),
            labels: HashMap::new(),
        };
        assert_eq!(spec.network_policy, NetworkPolicy::Disabled);
    }

    #[test]
    fn vm_spec_roundtrip_minimal() {
        let spec = VmSpec {
            backend_selector: BackendSelector::Auto,
            resources: VmResources::default(),
            network_policy: crate::sandbox::NetworkPolicy::Disabled,
            mounts: Vec::new(),
            env: HashMap::new(),
            runtime_hint: RuntimeHint::Shell,
            labels: HashMap::new(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let _back: VmSpec = serde_json::from_str(&json).unwrap();
    }

    // ── Handle / snapshot / fork ──

    #[test]
    fn vm_handle_roundtrip_preserves_metadata() {
        let handle = VmHandle {
            vm_id: VmId::from("vm-42"),
            backend: BackendId::from("local"),
            session_id: SessionId::from_string("sess-1"),
            agent_id: AgentId::from_string("agent-1"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::json!({ "region": "us-east-1" }),
        };
        let json = serde_json::to_string(&handle).unwrap();
        let back: VmHandle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.vm_id, handle.vm_id);
        assert_eq!(back.metadata["region"], "us-east-1");
    }

    #[test]
    fn vm_snapshot_handle_roundtrip() {
        let snap = VmSnapshotHandle {
            snapshot_id: VmSnapshotId::from("snap-1"),
            vm_id: VmId::from("vm-1"),
            name: "pre-fork".into(),
            created_at: Utc::now(),
            size_bytes: 1024,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: VmSnapshotHandle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, snap.name);
    }

    #[test]
    fn fork_spec_roundtrip() {
        let spec = ForkSpec {
            parent_snapshot: VmSnapshotId::from("snap-1"),
            overrides: VmSpecOverrides::default(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: ForkSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.parent_snapshot, spec.parent_snapshot);
    }

    // ── Exec ──

    #[test]
    fn exec_request_shell_helper() {
        let r = ExecRequest::shell("echo hi");
        assert_eq!(r.command, vec!["/bin/sh", "-c", "echo hi"]);
        assert!(r.working_dir.is_none());
        assert!(r.stdin.is_none());
    }

    #[test]
    fn exec_request_roundtrip_omits_none() {
        let r = ExecRequest::shell("true");
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("working_dir"));
        assert!(!json.contains("timeout_secs"));
        assert!(!json.contains("stdin"));
    }

    #[test]
    fn exec_result_roundtrip() {
        let r = ExecResult {
            stdout: b"hello".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
            duration_ms: 12,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ExecResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stdout, r.stdout);
        assert_eq!(back.exit_code, 0);
    }

    #[test]
    fn file_write_equality() {
        let a = FileWrite {
            path: "/tmp/a".into(),
            content: b"x".to_vec(),
            mode: 0o644,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
