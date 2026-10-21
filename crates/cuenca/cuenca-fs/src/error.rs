use cuenca_tree::{GitOid, PathError, TreeError};
use std::io;
use std::path::PathBuf;
use thiserror::Error;

/// Why a snapshot or a checkout failed. Every variant that concerns one file names it.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FsError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{0}: the name is not UTF-8, which a tree cannot hold")]
    NotUtf8(PathBuf),
    #[error("{path}: {source}")]
    Name { path: PathBuf, source: PathError },
    #[error("{path}: {size} bytes, over the {limit}-byte limit for one file")]
    TooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    #[error("{0}: SHA-1 collision detected in the content")]
    Collision(PathBuf),
    #[error("{0}: the file kept changing while it was read; snapshot again once it settles")]
    Unstable(PathBuf),
    #[error(
        "the base holds {} path(s) the rules name, which this snapshot would delete from the fork (first: {:?}); set SnapshotOptions::drop_ignored to accept that",
        .0.len(),
        &.0[..(.0.len().min(5))]
    )]
    Ignored(Vec<String>),
    #[error("the files cannot form a tree: {0}")]
    Tree(#[from] TreeError),
    #[error(".forkignore: {0}")]
    Ignore(String),
    #[error("{path}: a checkout needs a missing or empty directory of its own: {why}")]
    Destination { path: PathBuf, why: &'static str },
    #[error("no source has blob {0}")]
    MissingBlob(GitOid),
    #[error("the source returned content for {expected} that hashes to {actual}")]
    BlobMismatch { expected: GitOid, actual: GitOid },
    #[error("{0}: its source file changed while it was cloned")]
    SourceChanged(PathBuf),
}

impl FsError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}

pub(crate) type Result<T> = std::result::Result<T, FsError>;
