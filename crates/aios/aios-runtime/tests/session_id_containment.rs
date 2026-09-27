//! Session ids cannot steer a session's workspace outside `{root}/sessions/`
//! (BRO-1491).
//!
//! `create_session_with_id` turns the id into `{root}/sessions/<id>/`, creates
//! the workspace tree there and writes `manifest.json`; the path is then the
//! filesystem boundary of every tool call in the session. Before the guard, an
//! id of `../..` created directories and wrote the manifest in the parent of
//! the data dir, and the session's tools were rooted there.
//!
//! Contract under test, on the real runtime and the real filesystem:
//! 1. Traversal and malformed ids are rejected, and NOTHING is created
//!    outside the data dir (the side effect is asserted, not just the error).
//! 2. A session directory planted as a symlink out of the tree is rejected
//!    before a single file is written at the symlink's target.
//! 3. Legitimate ids still create a session whose workspace root is the
//!    canonical `{root}/sessions/<id>`.

use std::path::Path;
use std::sync::Arc;

use aios_protocol::{
    ApprovalId, ApprovalPort, ApprovalRequest, ApprovalResolution, ApprovalTicket, BranchId,
    Capability, EventRecord, EventRecordStream, EventStorePort, KernelResult, ModelCompletion,
    ModelCompletionRequest, ModelProviderPort, ModelRouting, ModelStopReason, PolicyGateDecision,
    PolicyGatePort, PolicySet, SessionId, ToolExecutionReport, ToolExecutionRequest,
    ToolHarnessPort,
};
use aios_runtime::{KernelRuntime, RuntimeConfig};
use async_trait::async_trait;

struct NullEventStore;

#[async_trait]
impl EventStorePort for NullEventStore {
    async fn append(&self, event: EventRecord) -> KernelResult<EventRecord> {
        Ok(event)
    }
    async fn read(
        &self,
        _session_id: SessionId,
        _branch_id: BranchId,
        _from_sequence: u64,
        _limit: usize,
    ) -> KernelResult<Vec<EventRecord>> {
        Ok(Vec::new())
    }
    async fn head(&self, _session_id: SessionId, _branch_id: BranchId) -> KernelResult<u64> {
        Ok(0)
    }
    async fn subscribe(
        &self,
        _session_id: SessionId,
        _branch_id: BranchId,
        _after_sequence: u64,
    ) -> KernelResult<EventRecordStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
}

struct NullProvider;

#[async_trait]
impl ModelProviderPort for NullProvider {
    async fn complete(&self, _request: ModelCompletionRequest) -> KernelResult<ModelCompletion> {
        Ok(ModelCompletion {
            provider: "null".to_owned(),
            model: "null".to_owned(),
            llm_call_record: None,
            directives: Vec::new(),
            stop_reason: ModelStopReason::Completed,
            usage: None,
            final_answer: None,
        })
    }
}

struct NullHarness;

#[async_trait]
impl ToolHarnessPort for NullHarness {
    async fn execute(&self, request: ToolExecutionRequest) -> KernelResult<ToolExecutionReport> {
        Err(aios_protocol::KernelError::ToolNotFound(
            request.call.tool_name,
        ))
    }
}

struct AllowAll;

#[async_trait]
impl PolicyGatePort for AllowAll {
    async fn evaluate(
        &self,
        _session_id: SessionId,
        requested: Vec<Capability>,
    ) -> KernelResult<PolicyGateDecision> {
        Ok(PolicyGateDecision {
            allowed: requested,
            requires_approval: Vec::new(),
            denied: Vec::new(),
        })
    }
}

struct NoApprovals;

#[async_trait]
impl ApprovalPort for NoApprovals {
    async fn enqueue(&self, request: ApprovalRequest) -> KernelResult<ApprovalTicket> {
        Ok(ApprovalTicket {
            approval_id: ApprovalId::default(),
            session_id: request.session_id,
            call_id: request.call_id,
            tool_name: request.tool_name,
            capability: request.capability,
            reason: request.reason,
            created_at: chrono::Utc::now(),
        })
    }
    async fn list_pending(&self, _session_id: SessionId) -> KernelResult<Vec<ApprovalTicket>> {
        Ok(Vec::new())
    }
    async fn resolve(
        &self,
        approval_id: ApprovalId,
        approved: bool,
        actor: String,
    ) -> KernelResult<ApprovalResolution> {
        Ok(ApprovalResolution {
            approval_id,
            approved,
            actor,
            resolved_at: chrono::Utc::now(),
        })
    }
}

fn runtime(data_dir: &Path) -> KernelRuntime {
    KernelRuntime::new(
        RuntimeConfig::new(data_dir.to_path_buf()),
        Arc::new(NullEventStore),
        Arc::new(NullProvider),
        Arc::new(NullHarness),
        Arc::new(NoApprovals),
        Arc::new(AllowAll),
    )
}

async fn create(
    runtime: &KernelRuntime,
    id: &str,
) -> anyhow::Result<aios_protocol::SessionManifest> {
    runtime
        .create_session_with_id(
            SessionId::from_string(id),
            "test",
            PolicySet::default(),
            ModelRouting::default(),
        )
        .await
}

/// Every path under `dir`, relative, sorted — the evidence that nothing was
/// created where it should not be.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(path.strip_prefix(base).unwrap().display().to_string());
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                walk(base, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[tokio::test]
async fn traversal_ids_are_rejected_without_side_effects() {
    let tmp = tempfile::tempdir().unwrap();
    // The data dir sits two levels down, so `../..` from `sessions/` lands in
    // `tmp` itself: the stand-in for the operator's home directory.
    let data_dir = tmp.path().join("home/data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(tmp.path().join("home/.bashrc"), "secret").unwrap();
    let runtime = runtime(&data_dir);
    let before = tree(tmp.path());

    let overlong = "x".repeat(aios_protocol::session_path::MAX_SESSION_ID_LEN + 1);
    let absolute = tmp.path().join("home").display().to_string();
    for id in [
        "",
        ".",
        "..",
        "../..",
        "../../..",
        "x/../../..",
        "x/../sess-b",
        absolute.as_str(),
        "/etc",
        "..\\..",
        "%2e%2e%2f%2e%2e",
        "..%2F..",
        "sess\u{FF0F}..",
        "sess\0a",
        overlong.as_str(),
    ] {
        let err = create(&runtime, id)
            .await
            .expect_err(&format!("{id:?} must be rejected"));
        assert!(
            err.downcast_ref::<aios_protocol::session_path::SessionPathError>()
                .is_some(),
            "{id:?} was rejected for the wrong reason: {err:#}"
        );
        assert!(
            !runtime.session_exists(&SessionId::from_string(id)),
            "{id:?} must not be registered"
        );
    }

    assert_eq!(
        tree(tmp.path()),
        before,
        "a rejected session id must not create or write anything"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_session_dir_is_rejected_before_any_write() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(data_dir.join("sessions")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    // e.g. planted by a shell command in another session.
    std::os::unix::fs::symlink(&outside, data_dir.join("sessions/victim")).unwrap();
    let runtime = runtime(&data_dir);

    let err = create(&runtime, "victim")
        .await
        .expect_err("symlink escape");
    assert!(
        matches!(
            err.downcast_ref::<aios_protocol::session_path::SessionPathError>(),
            Some(aios_protocol::session_path::SessionPathError::NotContained { .. })
        ),
        "wrong rejection: {err:#}"
    );
    assert!(
        tree(&outside).is_empty(),
        "nothing may be written through the symlink: {:?}",
        tree(&outside)
    );
}

#[tokio::test]
async fn legitimate_ids_create_contained_workspaces() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    let runtime = runtime(&data_dir);

    let max_len = "s".repeat(aios_protocol::session_path::MAX_SESSION_ID_LEN);
    for id in [
        "sess-a",
        "test_session",
        "3f2b8c1e-9d4a-4e7b-8c2f-1a5b6c7d8e9f",
        "01J9ZK3Q8X4V2N6M5B7C9D1E3F",
        max_len.as_str(),
    ] {
        let manifest = create(&runtime, id)
            .await
            .unwrap_or_else(|e| panic!("{id:?} must be accepted: {e:#}"));
        let expected = data_dir.join("sessions").join(id).canonicalize().unwrap();
        assert_eq!(Path::new(&manifest.workspace_root), expected);
        assert!(expected.join("manifest.json").is_file());
        assert!(runtime.session_exists(&SessionId::from_string(id)));
    }

    // A server-generated id (the no-id path) is a UUID and passes the grammar.
    let manifest = runtime
        .create_session("test", PolicySet::default(), ModelRouting::default())
        .await
        .unwrap();
    assert!(
        Path::new(&manifest.workspace_root)
            .join("manifest.json")
            .is_file()
    );
}
