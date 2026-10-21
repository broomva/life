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
use crate::names::{ForkName, LocationName};

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

/// A fork as the log has it now. Fields added later are `#[serde(default)]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Fork {
    pub name: ForkName,
    pub location: LocationName,
    /// The tree it started from.
    pub base: GitOid,
    /// The tree of its latest draft, or `base` before the first.
    pub head: GitOid,
    /// The latest draft's changeset and the offset of its record.
    pub head_changeset: Option<(ChangesetId, u64)>,
    /// The latest landing intent's changeset and the offset of its record.
    pub landing_intent: Option<(ChangesetId, u64)>,
}

/// What [`apply`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// The event changes the state; the caller appends it at the offset it gave.
    Fresh,
    /// The event repeats the latest record of its kind for the same fork, at this
    /// offset: a retry of a call that succeeded (a draft only while it is the head).     /// retry. The writer appends nothing; in a replay it is a duplicated record.
    Repeat(u64),
}

/// The state [`apply`] reads and writes.
pub(crate) trait View {
    fn location(&self, name: &LocationName) -> Result<Option<Location>>;
    fn fork(&self, name: &ForkName) -> Result<Option<Fork>>;
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
        } => {
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
                base: *base,
                head: *base,
                head_changeset: None,
                landing_intent: None,
            })?;
        }
        Event::DraftPublished {
            fork: name,
            id,
            changeset,
        } => {
            if ChangesetId::of(changeset) != *id {
                return Err(CoreError::Inconsistent(format!(
                    "draft {id} is not its changeset's id"
                )));
            }
            let mut state = fork(view, name)?;
            if let Some((head, at)) = state.head_changeset
                && head == *id
            {
                return Ok(Applied::Repeat(at));
            }
            if changeset.base != state.head {
                return Err(CoreError::NotAtBase {
                    what: format!("fork {name:?}"),
                    head: state.head,
                    base: changeset.base,
                });
            }
            state.head = changeset.target;
            state.head_changeset = Some((*id, offset));
            view.put_fork(&state)?;
            view.index_changeset(*id, offset, changeset)?;
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
            changeset: id,
        } => {
            let mut state = fork(view, name)?;
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

/// The projections held in memory, as a replay builds them.
#[derive(Debug, Default)]
pub(crate) struct Memory {
    pub(crate) locations: BTreeMap<LocationName, Location>,
    pub(crate) forks: BTreeMap<ForkName, Fork>,
    pub(crate) changesets: HashMap<ChangesetId, (u64, Changeset)>,
}

impl View for Memory {
    fn location(&self, name: &LocationName) -> Result<Option<Location>> {
        Ok(self.locations.get(name).cloned())
    }

    fn fork(&self, name: &ForkName) -> Result<Option<Fork>> {
        Ok(self.forks.get(name).cloned())
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
