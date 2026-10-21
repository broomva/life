use cuenca_tree::{Changeset, ContentHash, GitOid};
use serde::{Deserialize, Serialize};

use crate::names::{ForkName, LocationName, PrincipalName};
use crate::owner::{Holder, Writer};

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

/// One fact in the log. The locations, forks, changeset index and revocations are
/// projections of these events, rebuilt and compared by [`crate::Store::replay_check`].
///
/// The event set is not frozen yet: no log has been deployed, so a change needs no
/// migration; the format freezes when the authority first runs on the VPS (BRO-3257).
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
    /// A fork of a location, starting from the location's main at that moment, held by
    /// `holder` under epoch 1.
    ForkCreated {
        fork: ForkName,
        location: LocationName,
        base: GitOid,
        holder: Holder,
    },
    /// A draft: a changeset moving the fork's head from `changeset.base` to
    /// `changeset.target`, stored whole, written by the fork's holder under its
    /// current epoch. (A location's first tree is named by id only; its objects are in
    /// the store's tree table.)
    DraftPublished {
        fork: ForkName,
        writer: Writer,
        id: ChangesetId,
        changeset: Changeset,
    },
    /// A draft written under a stale epoch of `fork` by the holder of that epoch, kept
    /// as a draft of the sibling fork `sibling` ([`crate::sibling_name`]), which this
    /// event creates if it does not exist yet. Never dropped, never merged into `fork`.
    DraftFenced {
        fork: ForkName,
        sibling: ForkName,
        writer: Writer,
        id: ChangesetId,
        changeset: Changeset,
    },
    /// A fork's ownership moved to `holder`, by compare-and-swap on the epoch: the fork
    /// was at `expected_epoch`, and is now at `expected_epoch + 1`.
    TakenOver {
        fork: ForkName,
        expected_epoch: u64,
        holder: Holder,
    },
    /// A principal's keys were revoked: from here on the log refuses every write it
    /// asks for. Permanent; a new key means a new principal.
    PrincipalRevoked { principal: PrincipalName },
    /// A location's main moved by the changeset at the head of one of its forks. An
    /// authority act after the gates (landing, BRO-3255), so it names no writer.
    MainAdvanced {
        location: LocationName,
        fork: ForkName,
        changeset: ChangesetId,
    },
    /// The fork's holder asked to land the fork's head changeset; landing itself
    /// (BRO-3255) records what happened to it.
    LandingIntent {
        fork: ForkName,
        writer: Writer,
        changeset: ChangesetId,
    },
}

impl Event {
    /// The forks this event is about: the history of a fork is the records that name it.
    pub fn forks(&self) -> Vec<&ForkName> {
        match self {
            Event::LocationAdded { .. } | Event::PrincipalRevoked { .. } => vec![],
            Event::ForkCreated { fork, .. }
            | Event::DraftPublished { fork, .. }
            | Event::TakenOver { fork, .. }
            | Event::MainAdvanced { fork, .. }
            | Event::LandingIntent { fork, .. } => vec![fork],
            Event::DraftFenced { fork, sibling, .. } => vec![fork, sibling],
        }
    }
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
