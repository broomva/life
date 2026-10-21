use cuenca_tree::{ChangesetError, GitOid, HashFailure, PathError, TreeError};
use thiserror::Error;

use crate::names::NameError;

/// Why a store operation did not happen. Every refusal leaves the store as it was.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CoreError {
    #[error("log database: {0}")]
    Db(Box<redb::Error>),
    #[error("blob store: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Name(#[from] NameError),
    #[error(transparent)]
    Path(#[from] PathError),
    #[error(transparent)]
    Tree(#[from] TreeError),
    #[error(transparent)]
    Changeset(#[from] ChangesetError),
    #[error("blob content does not hash as git's: {0}")]
    Hash(HashFailure),
    #[error("blob claimed as {claimed} hashes to {actual}")]
    OidMismatch { claimed: GitOid, actual: GitOid },
    #[error("blob of {size} bytes is over the {limit}-byte limit")]
    TooLarge { size: u64, limit: u64 },
    #[error("blob {0} is not in the store")]
    MissingBlob(GitOid),
    #[error("blob {0} in the store no longer hashes to its id")]
    CorruptBlob(GitOid),
    #[error("tree {0} is not in the store")]
    MissingTree(GitOid),
    #[error("tree {0} in the store does not rebuild to its id")]
    CorruptTree(GitOid),
    #[error("no location {0:?}")]
    UnknownLocation(String),
    #[error("location {0:?} already exists")]
    LocationExists(String),
    #[error("no fork {0:?}")]
    UnknownFork(String),
    #[error("fork {0:?} already exists")]
    ForkExists(String),
    #[error("no changeset {0}")]
    UnknownChangeset(String),
    #[error("{what} is at tree {head}, but the changeset starts from {base}")]
    NotAtBase {
        what: String,
        head: GitOid,
        base: GitOid,
    },
    #[error("fork {fork:?} is not a fork of location {location:?}")]
    WrongLocation { fork: String, location: String },
    #[error("changeset {changeset} is not the head of fork {fork:?}")]
    NotHead { fork: String, changeset: String },
    #[error("fork name {0:?} contains \"-from\", which is kept for sibling forks")]
    ReservedName(String),
    #[error("principal {0:?} is revoked")]
    Revoked(String),
    #[error("{holder} does not hold fork {fork:?} under epoch {epoch}")]
    NotHolder {
        fork: String,
        holder: String,
        epoch: u64,
    },
    #[error("epoch {epoch} of fork {fork:?} is stale: the fork is at epoch {current}")]
    StaleEpoch {
        fork: String,
        epoch: u64,
        current: u64,
    },
    #[error("fork {fork:?} is at epoch {actual}, not {expected}")]
    EpochMismatch {
        fork: String,
        expected: u64,
        actual: u64,
    },
    #[error("remote {0:?} is not a plausible git remote")]
    BadRemote(String),
    #[error("log record {offset} is unreadable: {reason}")]
    CorruptLog { offset: u64, reason: String },
    #[error("the tables disagree with a replay of the log: {0}")]
    Inconsistent(String),
}

macro_rules! via_redb {
    ($($t:ty),*) => {$(
        impl From<$t> for CoreError {
            fn from(e: $t) -> Self {
                Self::Db(Box::new(e.into()))
            }
        }
    )*};
}
via_redb!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError
);

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
