use crate::oid::GitOid;
use thiserror::Error;

/// A path that cannot be a repository path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path {0:?} contains a NUL byte")]
    Nul(String),
    #[error("path {0:?} starts or ends with '/'")]
    Slash(String),
    #[error("path {0:?} has an empty, '.' or '..' component")]
    BadComponent(String),
    #[error("path {0:?} has a component git reserves (.git or an alias of it)")]
    GitDir(String),
    #[error("path is not UTF-8")]
    NotUtf8,
}

/// A set of entries that cannot form a tree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TreeError {
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("path {0:?} appears twice")]
    Duplicate(String),
    #[error("{file:?} is a file and also a directory of {inside:?}")]
    FileDirConflict { file: String, inside: String },
    #[error("{a:?} and {b:?} collide on a case- or normalisation-insensitive filesystem")]
    Collision { a: String, b: String },
    #[error("unsupported git mode {0}")]
    UnsupportedMode(String),
    #[error("bad object id {0:?}")]
    BadOid(String),
}

/// A changeset that cannot be built or applied.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChangesetError {
    #[error(transparent)]
    Tree(#[from] TreeError),
    #[error("changeset is based on tree {expected}, but the tree given is {actual}")]
    BaseMismatch { expected: GitOid, actual: GitOid },
    #[error("changeset should produce tree {expected}, but produced {actual}")]
    TargetMismatch { expected: GitOid, actual: GitOid },
    #[error("ops are not in canonical order at {0:?}: paths must strictly increase")]
    NotCanonical(String),
    #[error("delete of {0:?}, which the base does not have")]
    DeleteMissing(String),
    #[error("put of {0:?} changes nothing")]
    NoOpPut(String),
}
