use crate::oid::GitOid;
use thiserror::Error;

/// A path that cannot be a repository path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path or component is {0} bytes, over the limit")]
    TooLong(usize),
    #[error("path {0:?} contains a NUL byte")]
    Nul(String),
    #[error("path {0:?} contains a backslash")]
    Backslash(String),
    #[error("path {0:?} starts or ends with '/'")]
    Slash(String),
    #[error("path {0:?} has an empty, '.' or '..' component")]
    BadComponent(String),
    #[error("path {0:?} has a component git reserves (.git or an alias of it)")]
    GitDir(String),
    #[error("path is not UTF-8")]
    NotUtf8,
}

/// Why a symlink's target cannot be checked out as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum SymlinkProblem {
    #[error("the target is empty")]
    Empty,
    #[error("the target contains a NUL byte")]
    Nul,
    #[error("the target is {0} bytes, over {max}", max = crate::tree::MAX_SYMLINK_TARGET)]
    TooLong(usize),
}

/// A set of entries that cannot form a tree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TreeError {
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("path {0:?} appears twice")]
    Duplicate(String),
    #[error("{file:?} is a file and also a directory of {inside:?}")]
    FileDirConflict { file: String, inside: String },
    #[error("{a:?} and {b:?} collide on a case- or normalisation-insensitive filesystem")]
    Collision { a: String, b: String },
    #[error(
        "{0:?} names .gitmodules (under some alias); the format has no submodules, and git refuses or fscks such entries"
    )]
    Gitmodules(String),
    #[error(
        "{0:?} names .gitattributes (under some alias) but is not a regular file: git's fsck rejects a directory there, and a symlink is refused here too"
    )]
    GitattributesNotAFile(String),
    #[error("symlink {path:?} cannot be checked out as stored: {problem}")]
    SymlinkTarget {
        path: String,
        problem: SymlinkProblem,
    },
    #[error(".gitattributes at {path:?} breaks git's fsck limits: {why}")]
    GitattributesContent { path: String, why: String },
    #[error("unsupported git mode {0}")]
    UnsupportedMode(String),
    #[error("git tree has an empty directory {0:?}, which a checkout cannot hold")]
    EmptyDirectory(String),
    #[error("bad object id {0:?}")]
    BadOid(String),
}

/// A changeset that cannot be built or applied.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ChangesetError {
    #[error(transparent)]
    Tree(#[from] TreeError),
    #[error("changeset format {0} is not supported (this build reads format 1)")]
    Format(u32),
    #[error("changeset is based on tree {expected}, but the tree given is {actual}")]
    BaseMismatch { expected: GitOid, actual: GitOid },
    #[error("changeset should produce tree {expected}, but produced {actual}")]
    TargetMismatch { expected: GitOid, actual: GitOid },
    #[error("ops are not in canonical order at {0:?}: paths must strictly increase")]
    NotCanonical(String),
    #[error("delete of {0:?}, which the base does not have")]
    DeleteMissing(String),
    #[error("put of {0:?} changes nothing in git")]
    NoOpPut(String),
}
