//! The log's transition rules, in one place.
//!
//! [`apply`] is the only code that decides whether an event may follow the state the
//! log is in, and what state it leaves. Admission checks that need blob or tree
//! content (blob presence, content rules, a draft's ops and target) are the writer's,
//! in `store.rs`, and run before an event reaches these rules. The store's writer runs it against the redb
//! tables inside the transaction that appends the event; [`crate::Store::replay_check`]
//! runs it against an empty in-memory [`Memory`] for every record in order. The two
//! cannot disagree about a rule, because there is one copy of each.

use std::collections::{BTreeMap, HashMap};

use cuenca_tree::{Changeset, GitOid};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::event::{ChangesetId, Event};
use crate::names::{ForkName, LocationName, PrincipalName};
use crate::owner::{Holder, Writer, sibling_name};

/// A location as the log has it now. Fields added later are `#[serde(default)]`, so
/// rows written by this version still decode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Location {
    pub name: LocationName,
    /// The git remote it lands to, if any (section 6 of the spec). Landing (BRO-3255)
    /// decides which transports it accepts.
    pub remote: Option<String>,
    /// The tree of its main.
    pub main: GitOid,
    /// The last move of main, if any.
    pub main_moved: Option<MainMove>,
}

/// One move of a location's main.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MainMove {
    pub changeset: ChangesetId,
    /// The fork whose head it was.
    pub fork: ForkName,
    /// The offset of the `MainAdvanced` record.
    pub offset: u64,
}

/// A fork as the log has it now. Until the format freezes (BRO-3257) fields change
/// freely; after that, fields added later are `#[serde(default)]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Fork {
    pub name: ForkName,
    pub location: LocationName,
    /// Who held it, epoch by epoch: `holders[e - 1]` held it under epoch `e`. The last
    /// is the holder now, and the fork's epoch is the length (never 0).
    pub holders: Vec<Holder>,
    /// The head each finished epoch ended at: `released[e - 1]` is the tree the fork
    /// held when epoch `e` was taken over (one entry per takeover, so one fewer than
    /// `holders`). A stale writer's offline chain resumes from there and nowhere else.
    pub released: Vec<GitOid>,
    /// For a sibling fork, the fork whose stale-epoch publishes it keeps.
    pub parent: Option<ForkName>,
    /// The tree it started from: main for a fork, the head its parent's stale epoch left
    /// for a sibling ([`Fork::released_at`]).
    pub base: GitOid,
    /// The tree of its latest draft, or `base` before the first.
    pub head: GitOid,
    /// The latest draft's changeset and the offset of its record.
    pub head_changeset: Option<(ChangesetId, u64)>,
    /// The latest landing intent's changeset and the offset of its record.
    pub landing_intent: Option<(ChangesetId, u64)>,
}

impl Fork {
    /// The fencing epoch: 1 at creation, plus one per takeover.
    pub fn epoch(&self) -> u64 {
        self.holders.len() as u64
    }

    /// Who holds it now (`None` only for a damaged row; the log always gives a holder).
    pub fn holder(&self) -> Option<&Holder> {
        self.holders.last()
    }

    /// Who held it under `epoch`.
    pub fn holder_at(&self, epoch: u64) -> Option<&Holder> {
        let index = usize::try_from(epoch.checked_sub(1)?).ok()?;
        self.holders.get(index)
    }

    /// The head the fork held when `epoch` ended (`None` while it is current).
    pub fn released_at(&self, epoch: u64) -> Option<GitOid> {
        let ended = usize::try_from(epoch.checked_sub(1)?).ok()?;
        self.released.get(ended).copied()
    }
}

/// What [`apply`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// The event changes the state; the caller appends it at the offset it gave.
    Fresh,
    /// The event repeats the latest record of its kind for the same fork, at this
    /// offset: a retry of a call that succeeded (a draft only while it is the head).
    /// The writer appends nothing; in a replay it is a duplicated record.
    Repeat(u64),
}

/// The state [`apply`] reads and writes.
pub(crate) trait View {
    fn location(&self, name: &LocationName) -> Result<Option<Location>>;
    fn fork(&self, name: &ForkName) -> Result<Option<Fork>>;
    /// The offset of the record that revoked `principal`, if one did.
    fn revoked(&self, principal: &PrincipalName) -> Result<Option<u64>>;
    fn put_revoked(&mut self, principal: &PrincipalName, offset: u64) -> Result<()>;
    /// A changeset some draft recorded.
    fn changeset(&self, id: ChangesetId) -> Result<Option<Changeset>>;
    fn put_location(&mut self, location: &Location) -> Result<()>;
    fn put_fork(&mut self, fork: &Fork) -> Result<()>;
    /// Index a draft's changeset under its id, unless an earlier draft already did.
    fn index_changeset(
        &mut self,
        id: ChangesetId,
        offset: u64,
        changeset: &Changeset,
    ) -> Result<()>;
}

fn location(view: &impl View, name: &LocationName) -> Result<Location> {
    view.location(name)?
        .ok_or_else(|| CoreError::UnknownLocation(name.to_string()))
}

fn fork(view: &impl View, name: &ForkName) -> Result<Fork> {
    view.fork(name)?
        .ok_or_else(|| CoreError::UnknownFork(name.to_string()))
}

fn not_revoked(view: &impl View, principal: &PrincipalName) -> Result<()> {
    match view.revoked(principal)? {
        Some(_) => Err(CoreError::Revoked(principal.to_string())),
        None => Ok(()),
    }
}

/// Whether `writer` may write `state` now. A writer whose principal is revoked is
/// refused first; then the holder under the fork's current epoch may write; the holder
/// of an earlier epoch is told its epoch is stale (its publish goes to a sibling fork);
/// anyone else, or any epoch the fork has not reached, is not the holder.
pub(crate) fn check_writer(view: &impl View, state: &Fork, writer: &Writer) -> Result<()> {
    not_revoked(view, &writer.holder.principal)?;
    let current = state.epoch();
    if state.holder_at(writer.epoch) != Some(&writer.holder) {
        return Err(CoreError::NotHolder {
            fork: state.name.to_string(),
            holder: writer.holder.to_string(),
            epoch: writer.epoch,
        });
    }
    if writer.epoch != current {
        return Err(CoreError::StaleEpoch {
            fork: state.name.to_string(),
            epoch: writer.epoch,
            current,
        });
    }
    Ok(())
}

/// Apply `event`, to be recorded at `offset`, to `view`. On `Err` the view may be
/// partly written; the caller discards it (the writer aborts its transaction).
pub(crate) fn apply(view: &mut impl View, offset: u64, event: &Event) -> Result<Applied> {
    match event {
        Event::LocationAdded {
            location: name,
            remote,
            main,
        } => {
            if view.location(name)?.is_some() {
                return Err(CoreError::LocationExists(name.to_string()));
            }
            view.put_location(&Location {
                name: name.clone(),
                remote: remote.clone(),
                main: *main,
                main_moved: None,
            })?;
        }
        Event::ForkCreated {
            fork: name,
            location: at,
            base,
            holder,
        } => {
            not_revoked(view, &holder.principal)?;
            // `-from` is kept for siblings, so a sibling's name is never taken.
            if name.as_str().contains("-from") {
                return Err(CoreError::ReservedName(name.to_string()));
            }
            if view.fork(name)?.is_some() {
                return Err(CoreError::ForkExists(name.to_string()));
            }
            let loc = location(view, at)?;
            if *base != loc.main {
                return Err(CoreError::NotAtBase {
                    what: format!("main of {at:?}"),
                    head: loc.main,
                    base: *base,
                });
            }
            view.put_fork(&Fork {
                name: name.clone(),
                location: at.clone(),
                holders: vec![holder.clone()],
                released: Vec::new(),
                parent: None,
                base: *base,
                head: *base,
                head_changeset: None,
                landing_intent: None,
            })?;
        }
        Event::DraftPublished {
            fork: name,
            writer,
            id,
            changeset,
        } => {
            check_id(id, changeset)?;
            let state = fork(view, name)?;
            // Fencing comes before the retry: a fenced-out holder is never told Ok.
            check_writer(view, &state, writer)?;
            return draft(view, offset, state, id, changeset);
        }
        Event::DraftFenced {
            fork: name,
            sibling,
            writer,
            id,
            changeset,
        } => {
            check_id(id, changeset)?;
            let parent = fork(view, name)?;
            match check_writer(view, &parent, writer) {
                Err(CoreError::StaleEpoch { .. }) => {}
                Err(e) => return Err(e),
                Ok(()) => {
                    return Err(CoreError::Inconsistent(format!(
                        "draft {id} is fenced from {name:?}, but its writer holds it"
                    )));
                }
            }
            let state = fenced_target(view, &parent, writer, changeset.base)?;
            if state.name != *sibling {
                return Err(CoreError::Inconsistent(format!(
                    "draft {id} fenced from {name:?} belongs in {:?}, not {sibling:?}",
                    state.name
                )));
            }
            return draft(view, offset, state, id, changeset);
        }
        Event::TakenOver {
            fork: name,
            expected_epoch,
            holder,
        } => {
            not_revoked(view, &holder.principal)?;
            let mut state = fork(view, name)?;
            if state.epoch() != *expected_epoch {
                return Err(CoreError::EpochMismatch {
                    fork: name.to_string(),
                    expected: *expected_epoch,
                    actual: state.epoch(),
                });
            }
            state.released.push(state.head);
            state.holders.push(holder.clone());
            // A landing intent is the old holder's request; it does not pass to the new one.
            state.landing_intent = None;
            view.put_fork(&state)?;
        }
        Event::PrincipalRevoked { principal } => {
            if let Some(at) = view.revoked(principal)? {
                return Ok(Applied::Repeat(at));
            }
            view.put_revoked(principal, offset)?;
        }
        Event::MainAdvanced {
            location: at,
            fork: from,
            changeset: id,
        } => {
            let state = fork(view, from)?;
            if state.location != *at {
                return Err(CoreError::WrongLocation {
                    fork: from.to_string(),
                    location: at.to_string(),
                });
            }
            let mut loc = location(view, at)?;
            if let Some(last) = &loc.main_moved
                && last.changeset == *id
                && last.fork == *from
            {
                return Ok(Applied::Repeat(last.offset));
            }
            if state.head_changeset.map(|(head, _)| head) != Some(*id) {
                return Err(CoreError::NotHead {
                    fork: from.to_string(),
                    changeset: id.to_hex(),
                });
            }
            let changeset = view
                .changeset(*id)?
                .ok_or_else(|| CoreError::UnknownChangeset(id.to_hex()))?;
            if changeset.base != loc.main {
                return Err(CoreError::NotAtBase {
                    what: format!("main of {at:?}"),
                    head: loc.main,
                    base: changeset.base,
                });
            }
            loc.main = changeset.target;
            loc.main_moved = Some(MainMove {
                changeset: *id,
                fork: from.clone(),
                offset,
            });
            view.put_location(&loc)?;
        }
        Event::LandingIntent {
            fork: name,
            writer,
            changeset: id,
        } => {
            let mut state = fork(view, name)?;
            check_writer(view, &state, writer)?;
            if let Some((last, at)) = state.landing_intent
                && last == *id
            {
                return Ok(Applied::Repeat(at));
            }
            if state.head_changeset.map(|(head, _)| head) != Some(*id) {
                return Err(CoreError::NotHead {
                    fork: name.to_string(),
                    changeset: id.to_hex(),
                });
            }
            state.landing_intent = Some((*id, offset));
            view.put_fork(&state)?;
        }
    }
    Ok(Applied::Fresh)
}

/// The fork a draft fenced from `parent` (written by `writer` under one of `parent`'s
/// stale epochs) goes to: the sibling for that parent and epoch
/// ([`sibling_name`]), new if it does not exist yet, starting from the head the stale
/// epoch ended at ([`Fork::released_at`]) and held by the writer under epoch 1. A draft
/// whose base is any other tree is refused as `NotAtBase`: the writer could read only
/// what its epoch left, so a sibling never starts from a tree it was not given (another
/// location's main, another fork's head). If that sibling has since been taken over, the
/// writer is stale there too, and the draft goes one level down, to the sibling's own
/// sibling for epoch 1, and so on. So a stale draft always has a fork of the writer's
/// own to go to, never one someone else holds. Once there it is a draft like any
/// other: it must start from that fork's head (the writer's chain continues), or it
/// is refused as `NotAtBase` and stays in the writer's checkout, unacknowledged.
pub(crate) fn fenced_target(
    view: &impl View,
    parent: &Fork,
    writer: &Writer,
    base: GitOid,
) -> Result<Fork> {
    let (mut at, mut epoch) = (parent.clone(), writer.epoch);
    loop {
        let name = sibling_name(&at.name, epoch, &writer.holder.node);
        let Some(sibling) = view.fork(&name)? else {
            let released = at.released_at(epoch).ok_or_else(|| {
                CoreError::Inconsistent(format!("{:?} has no end for epoch {epoch}", at.name))
            })?;
            if base != released {
                return Err(CoreError::NotAtBase {
                    what: format!("fork {:?} as epoch {epoch} left it", at.name),
                    head: released,
                    base,
                });
            }
            return Ok(Fork {
                name,
                location: at.location.clone(),
                holders: vec![writer.holder.clone()],
                released: Vec::new(),
                parent: Some(at.name.clone()),
                base,
                head: base,
                head_changeset: None,
                landing_intent: None,
            });
        };
        // Only this rule creates a fork under a sibling's name (fork creation refuses
        // them), from this parent's holder under this epoch: the writer. A different
        // first holder means two (parent, epoch) pairs share a name (a hash collision):
        // refuse rather than mix two holders' drafts.
        if sibling.holder_at(1) != Some(&writer.holder) {
            return Err(CoreError::Inconsistent(format!(
                "sibling {name:?} was first held by another writer"
            )));
        }
        if sibling.epoch() == 1 {
            return Ok(sibling);
        }
        (at, epoch) = (sibling, 1);
    }
}

fn check_id(id: &ChangesetId, changeset: &Changeset) -> Result<()> {
    if ChangesetId::of(changeset) != *id {
        return Err(CoreError::Inconsistent(format!(
            "draft {id} is not its changeset's id"
        )));
    }
    Ok(())
}

/// Move `state`'s head by `changeset`, which must start from it. Publishing the
/// changeset that already is the head is a retry.
fn draft(
    view: &mut impl View,
    offset: u64,
    mut state: Fork,
    id: &ChangesetId,
    changeset: &Changeset,
) -> Result<Applied> {
    if let Some((head, at)) = state.head_changeset
        && head == *id
    {
        return Ok(Applied::Repeat(at));
    }
    if changeset.base != state.head {
        return Err(CoreError::NotAtBase {
            what: format!("fork {:?}", state.name),
            head: state.head,
            base: changeset.base,
        });
    }
    state.head = changeset.target;
    state.head_changeset = Some((*id, offset));
    view.put_fork(&state)?;
    view.index_changeset(*id, offset, changeset)?;
    Ok(Applied::Fresh)
}

/// The projections held in memory, as a replay builds them.
#[derive(Debug, Default)]
pub(crate) struct Memory {
    pub(crate) locations: BTreeMap<LocationName, Location>,
    pub(crate) forks: BTreeMap<ForkName, Fork>,
    pub(crate) changesets: HashMap<ChangesetId, (u64, Changeset)>,
    pub(crate) revoked: BTreeMap<PrincipalName, u64>,
}

impl View for Memory {
    fn location(&self, name: &LocationName) -> Result<Option<Location>> {
        Ok(self.locations.get(name).cloned())
    }

    fn fork(&self, name: &ForkName) -> Result<Option<Fork>> {
        Ok(self.forks.get(name).cloned())
    }

    fn revoked(&self, principal: &PrincipalName) -> Result<Option<u64>> {
        Ok(self.revoked.get(principal).copied())
    }

    fn put_revoked(&mut self, principal: &PrincipalName, offset: u64) -> Result<()> {
        self.revoked.insert(principal.clone(), offset);
        Ok(())
    }

    fn changeset(&self, id: ChangesetId) -> Result<Option<Changeset>> {
        Ok(self.changesets.get(&id).map(|(_, c)| c.clone()))
    }

    fn put_location(&mut self, location: &Location) -> Result<()> {
        self.locations
            .insert(location.name.clone(), location.clone());
        Ok(())
    }

    fn put_fork(&mut self, fork: &Fork) -> Result<()> {
        self.forks.insert(fork.name.clone(), fork.clone());
        Ok(())
    }

    fn index_changeset(
        &mut self,
        id: ChangesetId,
        offset: u64,
        changeset: &Changeset,
    ) -> Result<()> {
        self.changesets
            .entry(id)
            .or_insert_with(|| (offset, changeset.clone()));
        Ok(())
    }
}
