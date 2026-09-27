//! Session id → filesystem path containment (BRO-1491).
//!
//! A session id becomes a directory name (`{data_dir}/sessions/<id>/`), and
//! that directory becomes the filesystem boundary of every tool call the
//! session makes. An id is therefore untrusted input that selects a path, so
//! it is checked by two rules. The second re-runs the first, then adds a check
//! the grammar cannot make, because it looks at the filesystem:
//!
//! 1. [`validate_session_id`] — an allowlist grammar,
//!    `^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$`. An id that passes is exactly one
//!    normal path component on every platform: no separator, no `.`/`..`, no
//!    percent-encoding, no NUL, no non-ASCII look-alike, never empty. There is
//!    no blocklist to be incomplete; anything the grammar does not name is
//!    rejected.
//! 2. [`verify_session_root`] — canonical containment. Both the sessions
//!    directory and the candidate root are canonicalized (symlinks resolved)
//!    and the root must equal `canonical(sessions_dir)/<id>` exactly. A session
//!    directory that is a symlink to somewhere else, or a root naming a
//!    different session, fails here even when the id itself is well formed.

use std::path::{Path, PathBuf};

/// Longest accepted session id, in bytes (the grammar is ASCII-only).
pub const MAX_SESSION_ID_LEN: usize = 128;

/// Why a session id or a session workspace root was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionPathError {
    #[error("session id is empty")]
    Empty,
    #[error("session id is {len} bytes; the limit is {MAX_SESSION_ID_LEN}")]
    TooLong { len: usize },
    #[error(
        "session id {id:?} is not allowed: it must start with an ASCII letter or digit \
         and contain only ASCII letters, digits, '-' and '_'"
    )]
    Grammar { id: String },
    #[error("cannot resolve {path}: {reason}")]
    Unresolvable { path: String, reason: String },
    #[error("session workspace {root} is not the workspace of session {id:?} under {sessions_dir}")]
    NotContained {
        id: String,
        root: String,
        sessions_dir: String,
    },
}

/// Check `id` against the session-id grammar
/// `^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$`.
pub fn validate_session_id(id: &str) -> Result<(), SessionPathError> {
    let bytes = id.as_bytes();
    let Some(first) = bytes.first() else {
        return Err(SessionPathError::Empty);
    };
    if bytes.len() > MAX_SESSION_ID_LEN {
        return Err(SessionPathError::TooLong { len: bytes.len() });
    }
    let body_ok = bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_');
    if !first.is_ascii_alphanumeric() || !body_ok {
        return Err(SessionPathError::Grammar { id: id.to_owned() });
    }
    Ok(())
}

/// Verify that `root` is the workspace of session `id` under `sessions_dir`,
/// and return its canonical form.
///
/// Requires `id` to pass [`validate_session_id`], and both paths to exist.
/// Succeeds only when `canonicalize(root) == canonicalize(sessions_dir)/id`,
/// so a root reached through `..`, through a symlink, or naming another
/// session is rejected.
pub fn verify_session_root(
    sessions_dir: &Path,
    id: &str,
    root: &Path,
) -> Result<PathBuf, SessionPathError> {
    validate_session_id(id)?;
    let canonical_sessions = canonicalize(sessions_dir)?;
    let canonical_root = canonicalize(root)?;
    if canonical_root != canonical_sessions.join(id) {
        return Err(SessionPathError::NotContained {
            id: id.to_owned(),
            root: root.display().to_string(),
            sessions_dir: sessions_dir.display().to_string(),
        });
    }
    Ok(canonical_root)
}

fn canonicalize(path: &Path) -> Result<PathBuf, SessionPathError> {
    path.canonicalize()
        .map_err(|error| SessionPathError::Unresolvable {
            path: path.display().to_string(),
            reason: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legitimate_ids_are_accepted() {
        for id in [
            "a",
            "0",
            "default",
            "sess-a",
            "test_session",
            "S7",
            "01J9ZK3Q8X4V2N6M5B7C9D1E3F",           // ULID
            "3f2b8c1e-9d4a-4e7b-8c2f-1a5b6c7d8e9f", // UUID v4
            &"x".repeat(MAX_SESSION_ID_LEN),
        ] {
            assert_eq!(validate_session_id(id), Ok(()), "{id:?} must be accepted");
        }
    }

    #[test]
    fn traversal_and_malformed_ids_are_rejected() {
        let overlong = "x".repeat(MAX_SESSION_ID_LEN + 1);
        for id in [
            "",
            ".",
            "..",
            "../..",
            "../../outside",
            "x/../sess-b",
            "/etc",
            "/",
            "a/b",
            "a\\b",
            "..\\..",
            "%2e%2e%2f",
            "..%2F..",
            "%2F",
            "sess\0a",
            "sess a",
            "sess.a",
            "sess:a",
            "-leading-dash",
            "_leading_underscore",
            "sess\u{FF0F}a", // fullwidth solidus
            "sess\u{2215}a", // division slash
            "séance",
            overlong.as_str(),
        ] {
            assert!(
                validate_session_id(id).is_err(),
                "{id:?} must be rejected by the grammar"
            );
        }
    }

    fn sessions_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("data/sessions");
        std::fs::create_dir_all(sessions.join("sess-a")).unwrap();
        std::fs::create_dir_all(sessions.join("sess-b")).unwrap();
        (tmp, sessions)
    }

    #[test]
    fn own_workspace_is_contained() {
        let (_tmp, sessions) = sessions_fixture();
        let verified = verify_session_root(&sessions, "sess-a", &sessions.join("sess-a")).unwrap();
        assert_eq!(verified, sessions.join("sess-a").canonicalize().unwrap());
    }

    #[test]
    fn another_sessions_workspace_is_not_contained() {
        let (_tmp, sessions) = sessions_fixture();
        let err = verify_session_root(&sessions, "sess-a", &sessions.join("sess-b")).unwrap_err();
        assert!(
            matches!(err, SessionPathError::NotContained { .. }),
            "{err}"
        );
    }

    #[test]
    fn dotdot_roots_are_not_contained() {
        let (_tmp, sessions) = sessions_fixture();
        for root in [
            sessions.join("sess-a/../sess-b"),
            sessions.join("sess-a/.."),
            sessions.join("sess-a/../.."),
            sessions.join("../.."),
        ] {
            let err = verify_session_root(&sessions, "sess-a", &root).unwrap_err();
            assert!(
                matches!(err, SessionPathError::NotContained { .. }),
                "{}: {err}",
                root.display()
            );
        }
    }

    #[test]
    fn nested_directory_is_not_the_workspace() {
        let (_tmp, sessions) = sessions_fixture();
        std::fs::create_dir_all(sessions.join("sess-a/artifacts")).unwrap();
        let err = verify_session_root(&sessions, "sess-a", &sessions.join("sess-a/artifacts"))
            .unwrap_err();
        assert!(
            matches!(err, SessionPathError::NotContained { .. }),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_session_dir_escaping_the_tree_is_not_contained() {
        let (tmp, sessions) = sessions_fixture();
        let outside = tmp.path().join("home");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, sessions.join("victim")).unwrap();

        let err = verify_session_root(&sessions, "victim", &sessions.join("victim")).unwrap_err();
        assert!(
            matches!(err, SessionPathError::NotContained { .. }),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_another_session_is_not_contained() {
        let (_tmp, sessions) = sessions_fixture();
        std::os::unix::fs::symlink(sessions.join("sess-b"), sessions.join("alias")).unwrap();

        let err = verify_session_root(&sessions, "alias", &sessions.join("alias")).unwrap_err();
        assert!(
            matches!(err, SessionPathError::NotContained { .. }),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_data_dir_is_resolved_on_both_sides() {
        // A data dir reached through a symlink (e.g. macOS /var → /private/var)
        // must not make a legitimate root look uncontained.
        let (tmp, sessions) = sessions_fixture();
        let link = tmp.path().join("data-link");
        std::os::unix::fs::symlink(tmp.path().join("data"), &link).unwrap();

        let verified =
            verify_session_root(&link.join("sessions"), "sess-a", &sessions.join("sess-a"))
                .unwrap();
        assert_eq!(verified, sessions.join("sess-a").canonicalize().unwrap());
    }

    #[test]
    fn missing_root_is_unresolvable() {
        let (_tmp, sessions) = sessions_fixture();
        let err = verify_session_root(&sessions, "ghost", &sessions.join("ghost")).unwrap_err();
        assert!(
            matches!(err, SessionPathError::Unresolvable { .. }),
            "{err}"
        );
    }

    #[test]
    fn malformed_id_fails_before_touching_the_filesystem() {
        let (_tmp, sessions) = sessions_fixture();
        let err =
            verify_session_root(&sessions, "../sess-b", &sessions.join("sess-b")).unwrap_err();
        assert!(matches!(err, SessionPathError::Grammar { .. }), "{err}");
    }
}
