//! Authenticated owner → memory directory containment (BRO-1491 follow-up).
//!
//! Memory outlives a session, so it cannot live in the session workspace. It
//! is scoped to the session's **owner**: the authenticated subject that
//! created the session. That subject is recorded once, server-side, as a
//! session → owner *binding*, and every memory reader and writer resolves its
//! directory through that binding. No request field ever selects the owner.
//!
//! Layout under the kernel data dir:
//!
//! ```text
//! {data_dir}/session-owners/<session_id>   file; content = owner id (the binding)
//! {data_dir}/owners/<owner_id>/memory/     that owner's memory files
//! {data_dir}/memory/                       legacy shared memory (single-user mode only)
//! ```
//!
//! A binding is written **before** the session is created, by every creator
//! on every plane, atomically, and it is final. An authenticated creator
//! writes its owner id; a plane with no authenticated principal writes the
//! [`UNOWNED_BINDING`] marker. So a session is never visible without its
//! final binding, and a tick never sees its session's owner change.
//!
//! The binding lives outside `sessions/<id>/` on purpose: the session
//! workspace is writable by the session's own file tools, so anything stored
//! there (including `manifest.json`'s `owner`) can be rewritten by the session
//! it describes.
//!
//! An owner id is untrusted input that selects a path, so it gets the same
//! two rules as a session id in [`crate::session_path`]:
//!
//! 1. [`validate_owner_id`] — the allowlist grammar
//!    `^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$`, exactly one normal path component.
//! 2. [`owner_memory_root`] — canonical containment: the owner directory must
//!    canonicalize to exactly `canonical(owners)/<owner>` *before* anything is
//!    created inside it, and the memory directory to exactly
//!    `canonical(owner_dir)/memory`. A symlinked owner or memory directory, or
//!    a case variant of an existing owner on a case-insensitive filesystem,
//!    is rejected.
//!
//! What this does not cover is the same residual as `session_path`: the shell
//! is not a filesystem boundary, so a shell command can reach any path the
//! daemon can, including other owners' memory and the binding files. Owner
//! scoping binds the memory *tools*, the prompt, and the run observer.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::session_path::{SessionPathError, validate_session_id};

/// Directory (under the data dir) holding one binding file per owned session.
pub const SESSION_OWNERS_DIR: &str = "session-owners";
/// Directory (under the data dir) holding one subdirectory per owner.
pub const OWNERS_DIR: &str = "owners";
/// Memory subdirectory inside an owner directory.
pub const OWNER_MEMORY_DIR: &str = "memory";
/// Legacy single-store memory directory (under the data dir).
pub const LEGACY_MEMORY_DIR: &str = "memory";

/// Why an owner id, binding, or owner directory was rejected.
///
/// Messages never carry server paths or the rejected owner string, so they
/// are safe to surface to a caller or a model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OwnerScopeError {
    #[error(
        "owner id is not allowed: it must be 1-128 ASCII letters, digits, '-' or '_', \
         starting with a letter or digit"
    )]
    InvalidOwner,
    #[error("invalid session id: {0}")]
    InvalidSession(SessionPathError),
    #[error("session is already bound to a different owner")]
    Conflict,
    #[error("session owner binding is unreadable or malformed")]
    CorruptBinding,
    #[error("owner scope directory is not contained where it must be")]
    NotContained,
    #[error("owner scope I/O failed: {0}")]
    Io(String),
}

impl OwnerScopeError {
    fn io(error: std::io::Error) -> Self {
        // `io::Error`'s Display holds the OS message only, never the path.
        Self::Io(error.kind().to_string())
    }
}

/// Check `owner` against the owner-id grammar (identical to the session-id
/// grammar: one normal path component, ASCII allowlist).
pub fn validate_owner_id(owner: &str) -> Result<(), OwnerScopeError> {
    validate_session_id(owner).map_err(|_| OwnerScopeError::InvalidOwner)
}

/// Where memory lives for a deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryLocation {
    /// Single-principal mode (no authentication configured): every session
    /// shares one directory. Never selected when any auth secret is set.
    Shared(PathBuf),
    /// Multi-tenant mode: each session's memory is its bound owner's
    /// directory; a session without a binding has no memory.
    PerOwner { data_dir: PathBuf },
}

impl MemoryLocation {
    /// Select the scope for a deployment. `multi_tenant` is true whenever any
    /// authentication secret is configured.
    pub fn for_deployment(data_dir: &Path, multi_tenant: bool) -> Self {
        if multi_tenant {
            Self::PerOwner {
                data_dir: data_dir.to_path_buf(),
            }
        } else {
            Self::Shared(data_dir.join(LEGACY_MEMORY_DIR))
        }
    }

    /// The memory directory for `session_id`, or `None` when the session has
    /// no memory (multi-tenant mode, no owner binding).
    pub fn resolve(&self, session_id: &str) -> Result<Option<PathBuf>, OwnerScopeError> {
        match self {
            Self::Shared(dir) => Ok(Some(dir.clone())),
            Self::PerOwner { data_dir } => match session_owner(data_dir, session_id)? {
                None => Ok(None),
                Some(owner) => owner_memory_root(data_dir, &owner).map(Some),
            },
        }
    }

    pub fn is_per_owner(&self) -> bool {
        matches!(self, Self::PerOwner { .. })
    }

    /// The data dir holding owner bindings, in multi-tenant mode.
    pub fn owner_data_dir(&self) -> Option<&Path> {
        match self {
            Self::Shared(_) => None,
            Self::PerOwner { data_dir } => Some(data_dir),
        }
    }
}

/// Binding content that marks a session as permanently unowned. The leading
/// `-` is outside the owner grammar, so no owner id can equal it.
pub const UNOWNED_BINDING: &str = "-unowned";

/// A session's binding, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// No binding yet: nobody has created this session since owner scoping.
    Absent,
    /// Created by a plane with no authenticated principal. Never gets memory,
    /// never becomes owned.
    Unowned,
    /// Created by this authenticated owner.
    Owner(String),
}

/// Record that `session_id` belongs to `owner`, **before** the session is
/// created.
///
/// Every creator on every plane writes a binding first, atomically, and a
/// binding is final. Whoever wins the write decides the session's owner for
/// good, so no session is ever visible unbound and no in-flight tick can see
/// its owner change. Idempotent for the same owner. A session bound to
/// anyone else, or claimed as unowned, is a [`OwnerScopeError::Conflict`].
pub fn bind_session_owner(
    data_dir: &Path,
    session_id: &str,
    owner: &str,
) -> Result<(), OwnerScopeError> {
    validate_owner_id(owner)?;
    write_binding(data_dir, session_id, owner)
}

/// Claim `session_id` as permanently unowned before a plane with no
/// authenticated principal (chronos wakes, the substrate plane,
/// unauthenticated HTTP) creates or drives it. Idempotent. A session already
/// owned by someone is a [`OwnerScopeError::Conflict`].
pub fn claim_unowned(data_dir: &Path, session_id: &str) -> Result<(), OwnerScopeError> {
    write_binding(data_dir, session_id, UNOWNED_BINDING)
}

/// Write `content` as the binding for `session_id` unless one exists; an
/// existing binding must already equal `content`.
fn write_binding(data_dir: &Path, session_id: &str, content: &str) -> Result<(), OwnerScopeError> {
    validate_session_id(session_id).map_err(OwnerScopeError::InvalidSession)?;
    let dir = data_dir.join(SESSION_OWNERS_DIR);
    fs::create_dir_all(data_dir).map_err(OwnerScopeError::io)?;
    create_dir_if_absent(&dir)?;
    let matches = |binding: &Binding| match binding {
        Binding::Absent => false,
        Binding::Unowned => content == UNOWNED_BINDING,
        Binding::Owner(owner) => owner == content,
    };
    // `read_binding` refuses a `session-owners` that is not a real directory
    // (a planted symlink), and it runs before anything is written below.
    let existing = read_binding(data_dir, session_id)?;
    if existing != Binding::Absent {
        return if matches(&existing) {
            Ok(())
        } else {
            Err(OwnerScopeError::Conflict)
        };
    }

    // Write a temp file, then hard-link it into place: `hard_link` fails if
    // the target exists, so two racing writers cannot both win and a reader
    // never sees a half-written binding. The temp name starts with '.', which
    // the session-id grammar rejects, so it can never be read as a binding.
    // pid + a process-wide counter keep concurrent writers' temp files apart
    // (a clock alone collides between threads).
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".bind-{session_id}-{}-{seq}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(OwnerScopeError::io)?;
        file.write_all(content.as_bytes())
            .map_err(OwnerScopeError::io)?;
        file.sync_all().map_err(OwnerScopeError::io)?;
        match fs::hard_link(&tmp, dir.join(session_id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                if matches(&read_binding(data_dir, session_id)?) {
                    Ok(())
                } else {
                    Err(OwnerScopeError::Conflict)
                }
            }
            Err(e) => Err(OwnerScopeError::io(e)),
        }
    })();
    let _ = fs::remove_file(&tmp);
    result
}

/// Read `session_id`'s binding.
///
/// A binding that exists but is not a regular file, or whose content is
/// neither the unowned marker nor a grammatical owner id, is an error (never
/// `Absent`): callers must fail closed on it.
pub fn read_binding(data_dir: &Path, session_id: &str) -> Result<Binding, OwnerScopeError> {
    validate_session_id(session_id).map_err(OwnerScopeError::InvalidSession)?;
    let dir = data_dir.join(SESSION_OWNERS_DIR);
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Binding::Absent),
        Err(e) => return Err(OwnerScopeError::io(e)),
        Ok(meta) if !meta.is_dir() => return Err(OwnerScopeError::NotContained),
        Ok(_) => {}
    }
    let path = dir.join(session_id);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Binding::Absent),
        Err(e) => return Err(OwnerScopeError::io(e)),
        Ok(meta) if !meta.is_file() => return Err(OwnerScopeError::CorruptBinding),
        Ok(_) => {}
    }
    let content = fs::read_to_string(&path).map_err(|_| OwnerScopeError::CorruptBinding)?;
    if content == UNOWNED_BINDING {
        return Ok(Binding::Unowned);
    }
    validate_owner_id(&content).map_err(|_| OwnerScopeError::CorruptBinding)?;
    Ok(Binding::Owner(content))
}

/// The owner bound to `session_id`, or `None` if the session is unowned or
/// has no binding. Fails closed on a malformed binding (see [`read_binding`]).
pub fn session_owner(data_dir: &Path, session_id: &str) -> Result<Option<String>, OwnerScopeError> {
    Ok(match read_binding(data_dir, session_id)? {
        Binding::Owner(owner) => Some(owner),
        Binding::Absent | Binding::Unowned => None,
    })
}

/// Create (if needed) and verify `owner`'s memory directory, returning its
/// canonical path.
///
/// The owner directory is verified *before* the memory directory is created
/// inside it, so a planted symlink never receives a write.
pub fn owner_memory_root(data_dir: &Path, owner: &str) -> Result<PathBuf, OwnerScopeError> {
    validate_owner_id(owner)?;
    let owners = data_dir.join(OWNERS_DIR);
    fs::create_dir_all(&owners).map_err(OwnerScopeError::io)?;
    let canonical_owners = owners.canonicalize().map_err(OwnerScopeError::io)?;

    let owner_dir = owners.join(owner);
    create_dir_if_absent(&owner_dir)?;
    let canonical_owner = owner_dir
        .canonicalize()
        .map_err(|_| OwnerScopeError::NotContained)?;
    if canonical_owner != canonical_owners.join(owner) {
        return Err(OwnerScopeError::NotContained);
    }

    let memory = canonical_owner.join(OWNER_MEMORY_DIR);
    create_dir_if_absent(&memory)?;
    let canonical_memory = memory
        .canonicalize()
        .map_err(|_| OwnerScopeError::NotContained)?;
    if canonical_memory != canonical_owner.join(OWNER_MEMORY_DIR) {
        return Err(OwnerScopeError::NotContained);
    }
    Ok(canonical_memory)
}

/// What [`adopt_legacy_memory`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AdoptReport {
    /// File names copied into the owner's memory.
    pub copied: Vec<String>,
    /// File names skipped because the owner already had a file of that name.
    pub skipped_existing: Vec<String>,
    /// Entries skipped because they are not regular files (dirs, symlinks).
    pub skipped_not_file: Vec<String>,
}

/// Copy the legacy shared memory (`{data_dir}/memory/`) into `owner`'s memory.
///
/// This is the migration path for memory written before owner scoping. The
/// legacy store was shared by every session, so which owner it belongs to is
/// an operator decision; this function never guesses. It copies regular files
/// only, never overwrites a file the owner already has, never follows
/// symlinks, and never deletes or modifies the legacy directory.
pub fn adopt_legacy_memory(data_dir: &Path, owner: &str) -> Result<AdoptReport, OwnerScopeError> {
    let target = owner_memory_root(data_dir, owner)?;
    let legacy = data_dir.join(LEGACY_MEMORY_DIR);
    let mut report = AdoptReport::default();
    let entries = match fs::read_dir(&legacy) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(report),
        Err(e) => return Err(OwnerScopeError::io(e)),
        Ok(entries) => entries,
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(OwnerScopeError::io)?;
        names.push(entry.file_name());
    }
    names.sort();
    for name in names {
        let display = name.to_string_lossy().into_owned();
        let source = legacy.join(&name);
        let meta = fs::symlink_metadata(&source).map_err(OwnerScopeError::io)?;
        if !meta.is_file() {
            report.skipped_not_file.push(display);
            continue;
        }
        let content = fs::read(&source).map_err(OwnerScopeError::io)?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target.join(&name))
        {
            Ok(mut file) => {
                file.write_all(&content).map_err(OwnerScopeError::io)?;
                report.copied.push(display);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                report.skipped_existing.push(display);
            }
            Err(e) => return Err(OwnerScopeError::io(e)),
        }
    }
    Ok(report)
}

fn create_dir_if_absent(path: &Path) -> Result<(), OwnerScopeError> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(OwnerScopeError::io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MALICIOUS_OWNERS: &[&str] = &[
        "",
        ".",
        "..",
        "../victim",
        "../../outside",
        "x/../victim",
        "/etc",
        "a/b",
        "a\\b",
        "%2e%2e%2f",
        "owner\0x",
        "user@example.com",
        "-dash",
        "séance",
        "owner\u{FF0F}x",
    ];

    fn data_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn malicious_owner_ids_are_rejected_everywhere_and_nothing_is_created() {
        let tmp = data_dir();
        let long = "x".repeat(129);
        for owner in MALICIOUS_OWNERS.iter().copied().chain([long.as_str()]) {
            assert_eq!(
                validate_owner_id(owner),
                Err(OwnerScopeError::InvalidOwner),
                "{owner:?}"
            );
            assert_eq!(
                owner_memory_root(tmp.path(), owner),
                Err(OwnerScopeError::InvalidOwner),
                "{owner:?}"
            );
            assert_eq!(
                bind_session_owner(tmp.path(), "sess-a", owner),
                Err(OwnerScopeError::InvalidOwner),
                "{owner:?}"
            );
            assert_eq!(
                adopt_legacy_memory(tmp.path(), owner),
                Err(OwnerScopeError::InvalidOwner),
                "{owner:?}"
            );
        }
        // Nothing escaped the owners dir: no sibling of `owners` was created.
        let mut top: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        top.sort();
        assert!(
            top.iter()
                .all(|n| n == OWNERS_DIR || n == SESSION_OWNERS_DIR),
            "unexpected entries at the data-dir root: {top:?}"
        );
        assert!(!tmp.path().join("victim").exists());
        assert!(!tmp.path().join("outside").exists());
    }

    #[test]
    fn a_binding_is_stable_and_cannot_be_rebound() {
        let tmp = data_dir();
        assert_eq!(session_owner(tmp.path(), "sess-a"), Ok(None));
        bind_session_owner(tmp.path(), "sess-a", "alice").unwrap();
        bind_session_owner(tmp.path(), "sess-a", "alice").unwrap(); // idempotent
        assert_eq!(
            bind_session_owner(tmp.path(), "sess-a", "bob"),
            Err(OwnerScopeError::Conflict)
        );
        assert_eq!(
            session_owner(tmp.path(), "sess-a"),
            Ok(Some("alice".into()))
        );
        // No temp files left behind.
        let entries: Vec<_> = fs::read_dir(tmp.path().join(SESSION_OWNERS_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("sess-a")]);
    }

    #[test]
    fn an_unowned_claim_is_final_and_never_yields_memory() {
        let tmp = data_dir();
        claim_unowned(tmp.path(), "sess-u").unwrap();
        claim_unowned(tmp.path(), "sess-u").unwrap(); // idempotent
        assert_eq!(read_binding(tmp.path(), "sess-u"), Ok(Binding::Unowned));
        assert_eq!(
            bind_session_owner(tmp.path(), "sess-u", "alice"),
            Err(OwnerScopeError::Conflict),
            "an unowned session can never become owned"
        );
        assert_eq!(session_owner(tmp.path(), "sess-u"), Ok(None));
        let scope = MemoryLocation::PerOwner {
            data_dir: tmp.path().to_path_buf(),
        };
        assert_eq!(scope.resolve("sess-u"), Ok(None));
        // And an owned session can never be claimed unowned.
        bind_session_owner(tmp.path(), "sess-o", "alice").unwrap();
        assert_eq!(
            claim_unowned(tmp.path(), "sess-o"),
            Err(OwnerScopeError::Conflict)
        );
        assert_eq!(
            session_owner(tmp.path(), "sess-o"),
            Ok(Some("alice".into()))
        );
    }

    /// Racing writers on one session: exactly one binding wins, every other
    /// writer sees `Conflict`, and the stored binding is the winner's. Many
    /// rounds, so the `hard_link` AlreadyExists path (not only the pre-check)
    /// is exercised.
    #[test]
    fn racing_writers_produce_exactly_one_final_binding() {
        let tmp = data_dir();
        let mut hard_link_losers = 0;
        for round in 0..200 {
            let sid = format!("race-{round}");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
            let handles: Vec<_> = (0..6)
                .map(|i| {
                    let dir = tmp.path().to_path_buf();
                    let sid = sid.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        let who = match i % 3 {
                            0 => "alice",
                            1 => "bob",
                            _ => UNOWNED_BINDING,
                        };
                        let result = if who == UNOWNED_BINDING {
                            claim_unowned(&dir, &sid)
                        } else {
                            bind_session_owner(&dir, &sid, who)
                        };
                        (who, result)
                    })
                })
                .collect();
            let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            let stored = read_binding(tmp.path(), &sid).unwrap();
            let winner = match &stored {
                Binding::Owner(o) => o.as_str(),
                Binding::Unowned => UNOWNED_BINDING,
                Binding::Absent => panic!("no binding after a race"),
            };
            for (who, result) in &outcomes {
                if *who == winner {
                    assert_eq!(result, &Ok(()), "round {round}: the winner's peers succeed");
                } else {
                    assert_eq!(
                        result,
                        &Err(OwnerScopeError::Conflict),
                        "round {round}: {who} lost"
                    );
                    hard_link_losers += 1;
                }
            }
        }
        assert!(hard_link_losers > 0);
    }

    #[test]
    fn traversal_session_ids_never_reach_the_binding_store() {
        let tmp = data_dir();
        for sid in ["../x", "..", "a/b", ""] {
            assert!(matches!(
                bind_session_owner(tmp.path(), sid, "alice"),
                Err(OwnerScopeError::InvalidSession(_))
            ));
            assert!(matches!(
                session_owner(tmp.path(), sid),
                Err(OwnerScopeError::InvalidSession(_))
            ));
        }
        assert!(!tmp.path().join("x").exists());
    }

    #[test]
    fn a_tampered_binding_fails_closed_not_open() {
        let tmp = data_dir();
        let dir = tmp.path().join(SESSION_OWNERS_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("sess-a"), "../victim").unwrap();
        assert_eq!(
            session_owner(tmp.path(), "sess-a"),
            Err(OwnerScopeError::CorruptBinding)
        );
        let scope = MemoryLocation::PerOwner {
            data_dir: tmp.path().to_path_buf(),
        };
        assert_eq!(
            scope.resolve("sess-a"),
            Err(OwnerScopeError::CorruptBinding)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_binding_is_rejected() {
        let tmp = data_dir();
        let dir = tmp.path().join(SESSION_OWNERS_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(tmp.path().join("elsewhere"), "bob").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("elsewhere"), dir.join("sess-a")).unwrap();
        assert_eq!(
            session_owner(tmp.path(), "sess-a"),
            Err(OwnerScopeError::CorruptBinding)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_binding_directory_is_rejected_by_reader_and_binder() {
        let tmp = data_dir();
        let planted = tmp.path().join("planted");
        fs::create_dir_all(&planted).unwrap();
        fs::write(planted.join("sess-a"), "bob").unwrap();
        std::os::unix::fs::symlink(&planted, tmp.path().join(SESSION_OWNERS_DIR)).unwrap();
        assert_eq!(
            session_owner(tmp.path(), "sess-a"),
            Err(OwnerScopeError::NotContained)
        );
        assert_eq!(
            bind_session_owner(tmp.path(), "sess-b", "alice"),
            Err(OwnerScopeError::NotContained)
        );
        assert!(
            !planted.join("sess-b").exists(),
            "nothing written through the link"
        );
    }

    #[test]
    fn owners_are_isolated_and_an_owner_is_stable_across_sessions() {
        let tmp = data_dir();
        bind_session_owner(tmp.path(), "sess-a1", "alice").unwrap();
        bind_session_owner(tmp.path(), "sess-a2", "alice").unwrap();
        bind_session_owner(tmp.path(), "sess-b1", "bob").unwrap();
        let scope = MemoryLocation::PerOwner {
            data_dir: tmp.path().to_path_buf(),
        };
        let a1 = scope.resolve("sess-a1").unwrap().unwrap();
        let a2 = scope.resolve("sess-a2").unwrap().unwrap();
        let b1 = scope.resolve("sess-b1").unwrap().unwrap();
        assert_eq!(a1, a2, "one owner, two sessions, one memory");
        assert_ne!(a1, b1, "two owners, two memories");
        assert!(a1.ends_with("owners/alice/memory"));
        assert!(b1.ends_with("owners/bob/memory"));
        // An unbound session has no memory in multi-tenant mode.
        assert_eq!(scope.resolve("sess-unbound"), Ok(None));
    }

    #[test]
    fn shared_scope_is_the_legacy_directory() {
        let tmp = data_dir();
        let scope = MemoryLocation::for_deployment(tmp.path(), false);
        assert_eq!(
            scope.resolve("any-session"),
            Ok(Some(tmp.path().join(LEGACY_MEMORY_DIR)))
        );
        assert!(MemoryLocation::for_deployment(tmp.path(), true).is_per_owner());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_owner_dir_is_rejected_before_any_write() {
        let tmp = data_dir();
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(tmp.path().join(OWNERS_DIR)).unwrap();
        std::os::unix::fs::symlink(&outside, tmp.path().join(OWNERS_DIR).join("alice")).unwrap();
        assert_eq!(
            owner_memory_root(tmp.path(), "alice"),
            Err(OwnerScopeError::NotContained)
        );
        assert!(
            !outside.join(OWNER_MEMORY_DIR).exists(),
            "nothing may be created through the planted link"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_memory_dir_is_rejected() {
        let tmp = data_dir();
        let bob = owner_memory_root(tmp.path(), "bob").unwrap();
        let alice_dir = tmp.path().join(OWNERS_DIR).join("alice");
        fs::create_dir_all(&alice_dir).unwrap();
        std::os::unix::fs::symlink(&bob, alice_dir.join(OWNER_MEMORY_DIR)).unwrap();
        assert_eq!(
            owner_memory_root(tmp.path(), "alice"),
            Err(OwnerScopeError::NotContained)
        );
    }

    /// On a case-insensitive filesystem (macOS default) `BOB` would open
    /// `bob`'s directory; the canonical check refuses it. Skipped where the
    /// filesystem is case-sensitive (the variant is then a distinct owner).
    #[test]
    fn a_case_variant_owner_never_opens_another_owners_memory() {
        let tmp = data_dir();
        let bob = owner_memory_root(tmp.path(), "bob").unwrap();
        fs::write(bob.join("secret.md"), "bob").unwrap();
        let insensitive = tmp.path().join(OWNERS_DIR).join("BOB").exists();
        if !insensitive {
            return;
        }
        assert_eq!(
            owner_memory_root(tmp.path(), "BOB"),
            Err(OwnerScopeError::NotContained)
        );
    }

    #[test]
    fn adopt_copies_without_clobbering_and_never_deletes_the_legacy_store() {
        let tmp = data_dir();
        let legacy = tmp.path().join(LEGACY_MEMORY_DIR);
        fs::create_dir_all(legacy.join("subdir")).unwrap();
        fs::write(legacy.join("notes.md"), "legacy notes").unwrap();
        fs::write(legacy.join("prefs.md"), "legacy prefs").unwrap();
        let target = owner_memory_root(tmp.path(), "alice").unwrap();
        fs::write(target.join("prefs.md"), "alice's own prefs").unwrap();

        let report = adopt_legacy_memory(tmp.path(), "alice").unwrap();
        assert_eq!(report.copied, vec!["notes.md".to_string()]);
        assert_eq!(report.skipped_existing, vec!["prefs.md".to_string()]);
        assert_eq!(report.skipped_not_file, vec!["subdir".to_string()]);
        assert_eq!(
            fs::read_to_string(target.join("notes.md")).unwrap(),
            "legacy notes"
        );
        assert_eq!(
            fs::read_to_string(target.join("prefs.md")).unwrap(),
            "alice's own prefs",
            "an owner's existing file is never overwritten"
        );
        // The legacy store is untouched.
        assert_eq!(
            fs::read_to_string(legacy.join("notes.md")).unwrap(),
            "legacy notes"
        );
        assert_eq!(
            fs::read_to_string(legacy.join("prefs.md")).unwrap(),
            "legacy prefs"
        );
        assert!(legacy.join("subdir").is_dir());
        // Adopting into bob does not leak alice's own file.
        adopt_legacy_memory(tmp.path(), "bob").unwrap();
        let bob = owner_memory_root(tmp.path(), "bob").unwrap();
        assert_eq!(
            fs::read_to_string(bob.join("prefs.md")).unwrap(),
            "legacy prefs"
        );
    }
}
