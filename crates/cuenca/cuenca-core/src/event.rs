use cuenca_tree::{Changeset, ContentHash, GitOid};
use serde::{Deserialize, Serialize};

use crate::names::{ForkName, LocationName};

/// A changeset's id: the SHA-256 of its canonical JSON. Changesets are canonical (ops
/// sorted, one per path, each a real change), so two equal changesets share an id, and
/// a publish retried while its draft is still the fork's head is recognised as the
/// same publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChangesetId(ContentHash);

impl ChangesetId {
    pub fn of(changeset: &Changeset) -> Self {
        let json = serde_json::to_vec(changeset).expect("a changeset always serializes");
        Self(ContentHash::of(&json))
    }

    pub fn from_hex(s: &str) -> Result<Self, cuenca_tree::TreeError> {
        ContentHash::from_hex(s).map(Self)
    }

    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

impl std::fmt::Display for ChangesetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// One fact in the log. The locations, forks and changeset index are projections of
/// these events, rebuilt and compared by [`crate::Store::replay_check`].
///
/// The event set is not frozen yet: ownership (holder, fencing epoch, takeover,
/// revocation, BRO-3249) adds the writer's holder and epoch to the events that write.
/// No log has been deployed, so that change needs no migration; the format freezes
/// when the authority first runs on the VPS (BRO-3257).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// A directory tree became a location, with `main` as its first tree.
    LocationAdded {
        location: LocationName,
        remote: Option<String>,
        main: GitOid,
    },
    /// A fork of a location, starting from the location's main at that moment.
    ForkCreated {
        fork: ForkName,
        location: LocationName,
        base: GitOid,
    },
    /// A draft: a changeset moving the fork's head from `changeset.base` to
    /// `changeset.target`, stored whole. (A location's first tree is named by id only;
    /// its objects are in the store's tree table.)
    DraftPublished {
        fork: ForkName,
        id: ChangesetId,
        changeset: Changeset,
    },
    /// A location's main moved by the changeset at the head of one of its forks.
    MainAdvanced {
        location: LocationName,
        fork: ForkName,
        changeset: ChangesetId,
    },
    /// The intent to land a fork's head changeset; landing itself (BRO-3255) records
    /// what happened to it.
    LandingIntent {
        fork: ForkName,
        changeset: ChangesetId,
    },
}

/// An event at its position in the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Position in the log, from 0 with no gaps.
    pub offset: u64,
    /// Wall-clock time of the append, in milliseconds since the Unix epoch. Informative
    /// only: order is the offset.
    pub at_ms: u64,
    pub event: Event,
}
