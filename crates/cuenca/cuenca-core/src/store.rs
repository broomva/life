use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use cuenca_tree::{
    Blob, Changeset, ContentHash, Entry, GitOid, Mode, Op, RepoPath, Tree, check_content,
    has_content_rule,
};
use redb::{
    Database, Durability, ReadableTable, ReadableTableMetadata, TableDefinition, WriteTransaction,
};
use serde::{Deserialize, Serialize};

use crate::blobs::BlobFiles;
use crate::error::{CoreError, Result};
use crate::event::{ChangesetId, Event, Record};
use crate::names::{ForkName, LocationName};

type Bytes = &'static [u8];

/// The log: offset → JSON [`Record`]. Append-only; nothing else writes it.
const LOG: TableDefinition<u64, Bytes> = TableDefinition::new("log");
/// Git tree objects: tree oid → JSON `[(mode, name, oid)]`. Shared across trees, as in git.
const TREES: TableDefinition<Bytes, Bytes> = TableDefinition::new("trees");
/// Ingested blobs: oid → SHA-256 (32 bytes) then size (8 bytes, little-endian).
const BLOBS: TableDefinition<Bytes, Bytes> = TableDefinition::new("blobs");
/// Projection: location name → JSON [`Location`].
const LOCATIONS: TableDefinition<&str, Bytes> = TableDefinition::new("locations");
/// Projection: fork name → JSON [`Fork`].
const FORKS: TableDefinition<&str, Bytes> = TableDefinition::new("forks");
/// Projection: changeset id → offset of the draft that first recorded it.
const CHANGESETS: TableDefinition<Bytes, u64> = TableDefinition::new("changesets");

/// A location as the log has it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub name: LocationName,
    /// The git remote it lands to, if any (section 6 of the spec).
    pub remote: Option<String>,
    /// The tree of its main.
    pub main: GitOid,
    /// The changeset that last moved main, if any has.
    pub main_changeset: Option<ChangesetId>,
}

/// A fork as the log has it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fork {
    pub name: ForkName,
    pub location: LocationName,
    /// The tree it started from.
    pub base: GitOid,
    /// The tree of its latest draft, or `base` before the first.
    pub head: GitOid,
    /// The latest draft's changeset and its offset in the log.
    pub head_changeset: Option<(ChangesetId, u64)>,
}

/// What a blob's ingest stored next to its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobInfo {
    /// Derived from the bytes the store verified, never from the sender.
    pub sha256: ContentHash,
    pub size: u64,
}

/// The result of a publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Published {
    pub id: ChangesetId,
    /// The offset of the draft's record.
    pub offset: u64,
    /// False when this exact changeset already was the fork's head: a retried publish,
    /// which appends nothing.
    pub fresh: bool,
}

/// What [`Store::replay_check`] rebuilt from the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaySummary {
    pub records: u64,
    pub locations: usize,
    pub forks: usize,
    pub changesets: usize,
}

/// The authority's state: the log, its projections and the blobs, in one directory.
///
/// Each mutation is one redb write transaction that appends exactly one record and
/// updates the projections it implies, so after a crash the store holds the record
/// and its effects, or neither. redb commits with immediate durability (fsync), so
/// a call that returned `Ok` survives a crash.
pub struct Store {
    db: Database,
    blobs: BlobFiles,
}

impl Store {
    /// Open (or create) the store in `dir`: `dir/log.redb` and `dir/blobs/`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let db = Database::create(dir.join("log.redb"))?;
        let txn = db.begin_write()?;
        txn.open_table(LOG)?;
        txn.open_table(TREES)?;
        txn.open_table(BLOBS)?;
        txn.open_table(LOCATIONS)?;
        txn.open_table(FORKS)?;
        txn.open_table(CHANGESETS)?;
        txn.commit()?;
        let blobs = BlobFiles::open(&dir.join("blobs"))?;
        Ok(Self { db, blobs })
    }

    /// A write transaction that is durable once its commit returns.
    fn begin(&self) -> Result<WriteTransaction> {
        let mut txn = self.db.begin_write()?;
        txn.set_durability(Durability::Immediate);
        Ok(txn)
    }

    // ---- blobs ------------------------------------------------------------------

    /// Ingest a blob that claims to be `claimed`, streaming `size` bytes from
    /// `content`. The bytes are hashed as git does (with SHA-1 collision detection);
    /// unless they are exactly `size` bytes hashing to `claimed`, nothing is stored.
    /// The SHA-256 is derived from the same verified bytes. Ingesting a blob the store
    /// already holds reads nothing and returns its record.
    pub fn ingest_blob(&self, claimed: GitOid, size: u64, content: impl Read) -> Result<BlobInfo> {
        if let Some(info) = self.blob_info(claimed)? {
            return Ok(info);
        }
        let blob = self.blobs.write(claimed, size, content)?;
        let txn = self.begin()?;
        txn.open_table(BLOBS)?
            .insert(claimed.as_bytes().as_slice(), encode_blob(&blob).as_slice())?;
        txn.commit()?;
        Ok(BlobInfo {
            sha256: blob.sha256(),
            size: blob.size(),
        })
    }

    pub fn blob_info(&self, oid: GitOid) -> Result<Option<BlobInfo>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(BLOBS)?;
        Ok(table
            .get(oid.as_bytes().as_slice())?
            .map(|v| decode_blob(v.value())))
    }

    /// A blob's content, verified against its id on the way out.
    pub fn read_blob(&self, oid: GitOid) -> Result<Vec<u8>> {
        if self.blob_info(oid)?.is_none() {
            return Err(CoreError::MissingBlob(oid));
        }
        self.blobs.read(oid)
    }

    // ---- writes -----------------------------------------------------------------

    /// Register a location whose main is `main`. Every blob `main` names must already
    /// be ingested. Registering is an owner act; this layer does not check who asks.
    pub fn add_location(
        &self,
        name: &LocationName,
        remote: Option<&str>,
        main: &Tree,
    ) -> Result<u64> {
        if let Some(remote) = remote {
            check_remote(remote)?;
        }
        let txn = self.begin()?;
        let offset;
        {
            let mut locations = txn.open_table(LOCATIONS)?;
            if locations.get(name.as_str())?.is_some() {
                return Err(CoreError::LocationExists(name.to_string()));
            }
            let blobs = txn.open_table(BLOBS)?;
            for (path, entry) in main.entries() {
                self.check_entry(&blobs, path, entry)?;
            }
            store_tree(&mut txn.open_table(TREES)?, main)?;
            let location = Location {
                name: name.clone(),
                remote: remote.map(str::to_string),
                main: main.git_tree_id(),
                main_changeset: None,
            };
            offset = append(
                &mut txn.open_table(LOG)?,
                Event::LocationAdded {
                    location: name.clone(),
                    remote: location.remote.clone(),
                    main: location.main,
                },
            )?;
            locations.insert(name.as_str(), to_json(&location).as_slice())?;
        }
        txn.commit()?;
        Ok(offset)
    }

    /// Fork `location` at its current main.
    pub fn create_fork(&self, fork: &ForkName, location: &LocationName) -> Result<u64> {
        let txn = self.begin()?;
        let offset;
        {
            let mut forks = txn.open_table(FORKS)?;
            if forks.get(fork.as_str())?.is_some() {
                return Err(CoreError::ForkExists(fork.to_string()));
            }
            let loc = get_location(&txn.open_table(LOCATIONS)?, location)?;
            let state = Fork {
                name: fork.clone(),
                location: location.clone(),
                base: loc.main,
                head: loc.main,
                head_changeset: None,
            };
            offset = append(
                &mut txn.open_table(LOG)?,
                Event::ForkCreated {
                    fork: fork.clone(),
                    location: location.clone(),
                    base: loc.main,
                },
            )?;
            forks.insert(fork.as_str(), to_json(&state).as_slice())?;
        }
        txn.commit()?;
        Ok(offset)
    }

    /// Publish a draft: `changeset` must start from the fork's head, every blob it puts
    /// must be ingested, and content with a rule (symlinks, `.gitattributes`) must pass
    /// it. Publishing the changeset that already is the fork's head is a retry: it
    /// appends nothing and returns the first publish's offset.
    pub fn publish_draft(&self, fork: &ForkName, changeset: &Changeset) -> Result<Published> {
        let id = ChangesetId::of(changeset);
        let txn = self.begin()?;
        let offset;
        {
            let mut forks = txn.open_table(FORKS)?;
            let mut state = get_fork(&forks, fork)?;
            if let Some((head, at)) = state.head_changeset
                && head == id
            {
                return Ok(Published {
                    id,
                    offset: at,
                    fresh: false,
                });
            }
            if changeset.base != state.head {
                return Err(CoreError::NotAtBase {
                    what: format!("fork {fork:?}"),
                    head: state.head,
                    base: changeset.base,
                });
            }
            let mut trees = txn.open_table(TREES)?;
            let target = changeset.apply(&load_tree(&trees, state.head)?)?;
            let blobs = txn.open_table(BLOBS)?;
            for op in &changeset.ops {
                if let Op::Put { path, entry } = op {
                    self.check_entry(&blobs, path, entry)?;
                }
            }
            store_tree(&mut trees, &target)?;
            offset = append(
                &mut txn.open_table(LOG)?,
                Event::DraftPublished {
                    fork: fork.clone(),
                    id,
                    changeset: changeset.clone(),
                },
            )?;
            let mut changesets = txn.open_table(CHANGESETS)?;
            if changesets.get(id.as_bytes().as_slice())?.is_none() {
                changesets.insert(id.as_bytes().as_slice(), offset)?;
            }
            state.head = changeset.target;
            state.head_changeset = Some((id, offset));
            forks.insert(fork.as_str(), to_json(&state).as_slice())?;
        }
        txn.commit()?;
        Ok(Published {
            id,
            offset,
            fresh: true,
        })
    }

    /// Move a location's main by a changeset in the log, as a compare-and-swap: the
    /// changeset must start from the main the location has now. Advancing by the
    /// changeset that last moved main is a retry and appends nothing.
    pub fn advance_main(&self, location: &LocationName, id: ChangesetId) -> Result<u64> {
        let changeset = self
            .changeset(id)?
            .ok_or_else(|| CoreError::UnknownChangeset(id.to_hex()))?;
        let txn = self.begin()?;
        let offset;
        {
            let mut locations = txn.open_table(LOCATIONS)?;
            let mut loc = get_location(&locations, location)?;
            let mut log = txn.open_table(LOG)?;
            if loc.main_changeset == Some(id) {
                return Ok(last_offset(&log)?.expect("a location implies a record"));
            }
            if changeset.base != loc.main {
                return Err(CoreError::NotAtBase {
                    what: format!("main of {location:?}"),
                    head: loc.main,
                    base: changeset.base,
                });
            }
            offset = append(
                &mut log,
                Event::MainAdvanced {
                    location: location.clone(),
                    changeset: id,
                },
            )?;
            loc.main = changeset.target;
            loc.main_changeset = Some(id);
            locations.insert(location.as_str(), to_json(&loc).as_slice())?;
        }
        txn.commit()?;
        Ok(offset)
    }

    /// Record the intent to land `id`, which must be the fork's head.
    pub fn record_landing_intent(&self, fork: &ForkName, id: ChangesetId) -> Result<u64> {
        let txn = self.begin()?;
        let offset;
        {
            let state = get_fork(&txn.open_table(FORKS)?, fork)?;
            if state.head_changeset.map(|(head, _)| head) != Some(id) {
                return Err(CoreError::NotHead {
                    fork: fork.to_string(),
                    changeset: id.to_hex(),
                });
            }
            offset = append(
                &mut txn.open_table(LOG)?,
                Event::LandingIntent {
                    fork: fork.clone(),
                    changeset: id,
                },
            )?;
        }
        txn.commit()?;
        Ok(offset)
    }

    // ---- reads ------------------------------------------------------------------

    pub fn location(&self, name: &LocationName) -> Result<Option<Location>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(LOCATIONS)?;
        table
            .get(name.as_str())?
            .map(|v| from_json(v.value()))
            .transpose()
    }

    pub fn locations(&self) -> Result<Vec<Location>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(LOCATIONS)?;
        table.iter()?.map(|r| from_json(r?.1.value())).collect()
    }

    pub fn fork(&self, name: &ForkName) -> Result<Option<Fork>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(FORKS)?;
        table
            .get(name.as_str())?
            .map(|v| from_json(v.value()))
            .transpose()
    }

    pub fn forks(&self) -> Result<Vec<Fork>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(FORKS)?;
        table.iter()?.map(|r| from_json(r?.1.value())).collect()
    }

    /// The tree a location's main is at.
    pub fn main_tree(&self, location: &LocationName) -> Result<Tree> {
        let txn = self.db.begin_read()?;
        let loc = get_location(&txn.open_table(LOCATIONS)?, location)?;
        load_tree(&txn.open_table(TREES)?, loc.main)
    }

    /// The tree a fork's head is at.
    pub fn fork_head(&self, fork: &ForkName) -> Result<Tree> {
        let txn = self.db.begin_read()?;
        let state = get_fork(&txn.open_table(FORKS)?, fork)?;
        load_tree(&txn.open_table(TREES)?, state.head)
    }

    /// Any tree the log has produced, by its git tree id.
    pub fn tree(&self, oid: GitOid) -> Result<Tree> {
        let txn = self.db.begin_read()?;
        load_tree(&txn.open_table(TREES)?, oid)
    }

    /// A changeset by id, from the draft that first recorded it.
    pub fn changeset(&self, id: ChangesetId) -> Result<Option<Changeset>> {
        let txn = self.db.begin_read()?;
        let Some(offset) = txn
            .open_table(CHANGESETS)?
            .get(id.as_bytes().as_slice())?
            .map(|v| v.value())
        else {
            return Ok(None);
        };
        match read_record(&txn.open_table(LOG)?, offset)?.event {
            Event::DraftPublished { changeset, .. } => Ok(Some(changeset)),
            other => Err(CoreError::CorruptLog {
                offset,
                reason: format!("changeset index points at {other:?}"),
            }),
        }
    }

    /// The number of records in the log.
    pub fn len(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Ok(txn.open_table(LOG)?.len()?)
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Up to `limit` records from `from` on.
    pub fn records(&self, from: u64, limit: usize) -> Result<Vec<Record>> {
        let txn = self.db.begin_read()?;
        let log = txn.open_table(LOG)?;
        log.range(from..)?
            .take(limit)
            .map(|r| {
                let (k, v) = r?;
                decode_record(k.value(), v.value())
            })
            .collect()
    }

    /// Rebuild every projection from the log alone and compare it with the tables.
    ///
    /// Checks that offsets run from 0 with no gap, that every record decodes, that
    /// every draft's id is its changeset's, starts from its fork's head at that point
    /// and applies, that no draft repeats its fork's head (a duplicated publish), that
    /// main only moves by known changesets from the tree it was at, and that the
    /// locations, forks and changeset index equal the replay's, with every tree and
    /// blob they reach present.
    pub fn replay_check(&self) -> Result<ReplaySummary> {
        let txn = self.db.begin_read()?;
        let log = txn.open_table(LOG)?;
        let trees = txn.open_table(TREES)?;
        let blobs = txn.open_table(BLOBS)?;
        let mut locations: BTreeMap<LocationName, Location> = BTreeMap::new();
        let mut forks: BTreeMap<ForkName, Fork> = BTreeMap::new();
        let mut changesets: HashMap<ChangesetId, (u64, Changeset)> = HashMap::new();
        let mut records = 0u64;
        let bad = |offset: u64, reason: String| CoreError::CorruptLog { offset, reason };
        for item in log.iter()? {
            let (k, v) = item?;
            let record = decode_record(k.value(), v.value())?;
            let offset = record.offset;
            if offset != records {
                return Err(bad(offset, format!("expected offset {records}")));
            }
            records += 1;
            match record.event {
                Event::LocationAdded {
                    location,
                    remote,
                    main,
                } => {
                    let tree = load_tree(&trees, main)?;
                    check_blobs(&blobs, &tree)?;
                    let fresh = Location {
                        name: location.clone(),
                        remote,
                        main,
                        main_changeset: None,
                    };
                    if locations.insert(location, fresh).is_some() {
                        return Err(bad(offset, "location added twice".into()));
                    }
                }
                Event::ForkCreated {
                    fork,
                    location,
                    base,
                } => {
                    let loc = locations
                        .get(&location)
                        .ok_or_else(|| bad(offset, format!("unknown location {location}")))?;
                    if loc.main != base {
                        return Err(bad(offset, "fork base is not main".into()));
                    }
                    let state = Fork {
                        name: fork.clone(),
                        location,
                        base,
                        head: base,
                        head_changeset: None,
                    };
                    if forks.insert(fork, state).is_some() {
                        return Err(bad(offset, "fork created twice".into()));
                    }
                }
                Event::DraftPublished {
                    fork,
                    id,
                    changeset,
                } => {
                    if ChangesetId::of(&changeset) != id {
                        return Err(bad(offset, "draft id is not its changeset's".into()));
                    }
                    let state = forks
                        .get_mut(&fork)
                        .ok_or_else(|| bad(offset, format!("unknown fork {fork}")))?;
                    if state.head_changeset.map(|(h, _)| h) == Some(id) {
                        return Err(bad(offset, format!("draft {id} duplicates the head")));
                    }
                    if changeset.base != state.head {
                        return Err(bad(offset, "draft does not start at the head".into()));
                    }
                    let target = changeset.apply(&load_tree(&trees, state.head)?)?;
                    check_blobs(&blobs, &target)?;
                    load_tree(&trees, changeset.target)?;
                    state.head = changeset.target;
                    state.head_changeset = Some((id, offset));
                    changesets.entry(id).or_insert((offset, changeset));
                }
                Event::MainAdvanced {
                    location,
                    changeset,
                } => {
                    let (_, cs) = changesets
                        .get(&changeset)
                        .ok_or_else(|| bad(offset, format!("unknown changeset {changeset}")))?;
                    let loc = locations
                        .get_mut(&location)
                        .ok_or_else(|| bad(offset, format!("unknown location {location}")))?;
                    if cs.base != loc.main {
                        return Err(bad(offset, "main moved from a tree it was not at".into()));
                    }
                    loc.main = cs.target;
                    loc.main_changeset = Some(changeset);
                }
                Event::LandingIntent { fork, changeset } => {
                    let state = forks
                        .get(&fork)
                        .ok_or_else(|| bad(offset, format!("unknown fork {fork}")))?;
                    if state.head_changeset.map(|(h, _)| h) != Some(changeset) {
                        return Err(bad(offset, "landing intent for a non-head".into()));
                    }
                }
            }
        }

        let differ = |what: &str| CoreError::Inconsistent(what.to_string());
        let table_locations: BTreeMap<LocationName, Location> = txn
            .open_table(LOCATIONS)?
            .iter()?
            .map(|r| {
                let loc: Location = from_json(r?.1.value())?;
                Ok((loc.name.clone(), loc))
            })
            .collect::<Result<_>>()?;
        if table_locations != locations {
            return Err(differ("locations"));
        }
        let table_forks: BTreeMap<ForkName, Fork> = txn
            .open_table(FORKS)?
            .iter()?
            .map(|r| {
                let fork: Fork = from_json(r?.1.value())?;
                Ok((fork.name.clone(), fork))
            })
            .collect::<Result<_>>()?;
        if table_forks != forks {
            return Err(differ("forks"));
        }
        let index = txn.open_table(CHANGESETS)?;
        if index.len()? != changesets.len() as u64 {
            return Err(differ("changeset index size"));
        }
        for (id, (offset, _)) in &changesets {
            if index.get(id.as_bytes().as_slice())?.map(|v| v.value()) != Some(*offset) {
                return Err(differ("changeset index entry"));
            }
        }
        Ok(ReplaySummary {
            records,
            locations: locations.len(),
            forks: forks.len(),
            changesets: changesets.len(),
        })
    }

    // ---- helpers ----------------------------------------------------------------

    /// An entry may enter the log only if its blob is ingested and, where a content
    /// rule applies, the blob's bytes pass it.
    fn check_entry(
        &self,
        blobs: &impl ReadableTable<Bytes, Bytes>,
        path: &RepoPath,
        entry: &Entry,
    ) -> Result<()> {
        let oid = entry.oid();
        if blobs.get(oid.as_bytes().as_slice())?.is_none() {
            return Err(CoreError::MissingBlob(oid));
        }
        if has_content_rule(path, entry.mode()) {
            check_content(path, entry.mode(), &self.blobs.read(oid)?)?;
        }
        Ok(())
    }
}

fn check_remote(remote: &str) -> Result<()> {
    let ok = !remote.is_empty()
        && remote.len() <= 512
        && !remote.starts_with('-')
        && remote.bytes().all(|b| b.is_ascii_graphic());
    if ok {
        Ok(())
    } else {
        Err(CoreError::BadRemote(remote.to_string()))
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn last_offset(log: &impl ReadableTable<u64, Bytes>) -> Result<Option<u64>> {
    Ok(log.last()?.map(|(k, _)| k.value()))
}

/// Append `event` at the next offset. Only the transaction holding the log's one
/// writer calls this, so offsets cannot race.
fn append(log: &mut redb::Table<u64, Bytes>, event: Event) -> Result<u64> {
    let offset = last_offset(log)?.map_or(0, |last| last + 1);
    let record = Record {
        offset,
        at_ms: now_ms(),
        event,
    };
    log.insert(offset, to_json(&record).as_slice())?;
    Ok(offset)
}

fn read_record(log: &impl ReadableTable<u64, Bytes>, offset: u64) -> Result<Record> {
    let value = log.get(offset)?.ok_or(CoreError::CorruptLog {
        offset,
        reason: "missing".into(),
    })?;
    decode_record(offset, value.value())
}

fn decode_record(offset: u64, bytes: &[u8]) -> Result<Record> {
    let record: Record = serde_json::from_slice(bytes).map_err(|e| CoreError::CorruptLog {
        offset,
        reason: e.to_string(),
    })?;
    if record.offset != offset {
        return Err(CoreError::CorruptLog {
            offset,
            reason: format!("record says offset {}", record.offset),
        });
    }
    Ok(record)
}

fn get_location(
    table: &impl ReadableTable<&'static str, Bytes>,
    name: &LocationName,
) -> Result<Location> {
    let value = table
        .get(name.as_str())?
        .ok_or_else(|| CoreError::UnknownLocation(name.to_string()))?;
    from_json(value.value())
}

fn get_fork(table: &impl ReadableTable<&'static str, Bytes>, name: &ForkName) -> Result<Fork> {
    let value = table
        .get(name.as_str())?
        .ok_or_else(|| CoreError::UnknownFork(name.to_string()))?;
    from_json(value.value())
}

fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("store types always serialize")
}

fn from_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| CoreError::Inconsistent(e.to_string()))
}

fn encode_blob(blob: &Blob) -> Vec<u8> {
    let mut v = blob.sha256().as_bytes().to_vec();
    v.extend_from_slice(&blob.size().to_le_bytes());
    v
}

fn decode_blob(bytes: &[u8]) -> BlobInfo {
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&bytes[..32]);
    let mut size = [0u8; 8];
    size.copy_from_slice(&bytes[32..40]);
    BlobInfo {
        sha256: ContentHash::from_bytes(sha),
        size: u64::from_le_bytes(size),
    }
}

/// One stored git tree object's children.
type Children = Vec<(String, String, GitOid)>;

/// Store every git tree object of `tree` that is not stored yet.
fn store_tree(trees: &mut redb::Table<Bytes, Bytes>, tree: &Tree) -> Result<()> {
    for object in tree.git_objects() {
        if trees.get(object.oid.as_bytes().as_slice())?.is_some() {
            continue;
        }
        let children: Children = object
            .children
            .iter()
            .map(|(mode, name, oid)| (mode.as_str().to_string(), name.clone(), *oid))
            .collect();
        trees.insert(
            object.oid.as_bytes().as_slice(),
            to_json(&children).as_slice(),
        )?;
    }
    Ok(())
}

/// Rebuild the tree `root` from its stored objects, and check that it hashes back to
/// `root`, so a damaged table cannot hand out a different tree under the same id.
fn load_tree(trees: &impl ReadableTable<Bytes, Bytes>, root: GitOid) -> Result<Tree> {
    let mut entries = Vec::new();
    let mut stack = vec![(String::new(), root)];
    while let Some((prefix, oid)) = stack.pop() {
        let value = trees
            .get(oid.as_bytes().as_slice())?
            .ok_or(CoreError::MissingTree(oid))?;
        let children: Children = from_json(value.value())?;
        for (mode, name, child) in children {
            let path = format!("{prefix}{name}");
            match Mode::parse(&mode)? {
                Mode::Tree => stack.push((format!("{path}/"), child)),
                mode => entries.push((RepoPath::new(path)?, Entry::from_mode(mode, child)?)),
            }
        }
    }
    let tree = Tree::from_entries(entries)?;
    if tree.git_tree_id() != root {
        return Err(CoreError::CorruptTree(root));
    }
    Ok(tree)
}

fn check_blobs(blobs: &impl ReadableTable<Bytes, Bytes>, tree: &Tree) -> Result<()> {
    for (_, entry) in tree.entries() {
        if blobs.get(entry.oid().as_bytes().as_slice())?.is_none() {
            return Err(CoreError::MissingBlob(entry.oid()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Store, Changeset) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let put = |c: &[u8]| {
            let oid = GitOid::for_blob(c);
            store.ingest_blob(oid, c.len() as u64, c).unwrap();
            Entry::File(oid)
        };
        let main = Tree::from_entries([(RepoPath::new("d/a").unwrap(), put(b"1"))]).unwrap();
        let l = LocationName::new("l").unwrap();
        let f = ForkName::new("f").unwrap();
        store.add_location(&l, None, &main).unwrap();
        store.create_fork(&f, &l).unwrap();
        let cs = Changeset::new(
            &main,
            vec![Op::put(RepoPath::new("d/a").unwrap(), put(b"2"))],
        )
        .unwrap();
        store.publish_draft(&f, &cs).unwrap();
        assert!(store.replay_check().is_ok());
        (dir, store, cs)
    }

    /// Write `event` into the log behind the store's back, at `offset`.
    fn inject(store: &Store, offset: u64, event: Event) {
        let txn = store.begin().unwrap();
        let record = Record {
            offset,
            at_ms: 0,
            event,
        };
        txn.open_table(LOG)
            .unwrap()
            .insert(offset, to_json(&record).as_slice())
            .unwrap();
        txn.commit().unwrap();
    }

    fn corrupt(store: &Store) -> String {
        match store.replay_check() {
            Err(CoreError::CorruptLog { reason, .. }) => reason,
            other => panic!("expected a corrupt log, got {other:?}"),
        }
    }

    #[test]
    fn replay_refuses_a_duplicated_draft() {
        let (_d, store, cs) = setup();
        let fork = ForkName::new("f").unwrap();
        let id = ChangesetId::of(&cs);
        inject(
            &store,
            3,
            Event::DraftPublished {
                fork,
                id,
                changeset: cs,
            },
        );
        assert!(corrupt(&store).contains("duplicates the head"));
    }

    #[test]
    fn replay_refuses_a_gap() {
        let (_d, store, _) = setup();
        let fork = ForkName::new("f").unwrap();
        let id = store
            .fork(&fork)
            .unwrap()
            .unwrap()
            .head_changeset
            .unwrap()
            .0;
        inject(
            &store,
            4,
            Event::LandingIntent {
                fork,
                changeset: id,
            },
        );
        assert!(corrupt(&store).contains("expected offset 3"));
    }

    #[test]
    fn replay_refuses_a_draft_under_a_wrong_id() {
        let (_d, store, cs) = setup();
        let fork = ForkName::new("f").unwrap();
        let back = Changeset::between(
            &store.fork_head(&fork).unwrap(),
            &store.tree(cs.base).unwrap(),
        );
        inject(
            &store,
            3,
            Event::DraftPublished {
                fork,
                id: ChangesetId::of(&cs),
                changeset: back,
            },
        );
        assert!(corrupt(&store).contains("not its changeset's"));
    }

    #[test]
    fn replay_refuses_tables_the_log_does_not_explain() {
        let (_d, store, _) = setup();
        let txn = store.begin().unwrap();
        {
            let mut forks = txn.open_table(FORKS).unwrap();
            let mut f: Fork = from_json(forks.get("f").unwrap().unwrap().value()).unwrap();
            f.head = f.base;
            forks.insert("f", to_json(&f).as_slice()).unwrap();
        }
        txn.commit().unwrap();
        assert!(matches!(store.replay_check(), Err(CoreError::Inconsistent(w)) if w == "forks"));
    }

    #[test]
    fn a_stored_tree_that_does_not_rebuild_to_its_id_is_refused() {
        let (_d, store, cs) = setup();
        // Point the root at different children: the subtree "d" now holds the base's
        // "a" under a new name.
        let txn = store.begin().unwrap();
        {
            let mut trees = txn.open_table(TREES).unwrap();
            let root = cs.target.as_bytes().as_slice();
            let mut children: Children =
                from_json(trees.get(root).unwrap().unwrap().value()).unwrap();
            children[0].1 = "e".into();
            trees.insert(root, to_json(&children).as_slice()).unwrap();
        }
        txn.commit().unwrap();
        assert!(matches!(store.tree(cs.target), Err(CoreError::CorruptTree(o)) if o == cs.target));
    }
}
