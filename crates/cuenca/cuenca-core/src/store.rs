use std::collections::{BTreeMap, HashSet};
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
use crate::state::{Applied, Fork, Location, Memory, View, apply};

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

/// What a blob's ingest stored next to its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobInfo {
    /// Derived from the bytes the store verified, never from the sender.
    pub sha256: ContentHash,
    pub size: u64,
}

/// The result of a publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Published {
    pub id: ChangesetId,
    /// The offset of the draft's record.
    pub offset: u64,
    /// False when this changeset already was the fork's head: a retried publish, which
    /// appends nothing and returns the first publish's offset.
    pub fresh: bool,
}

/// What [`Store::replay_check`] rebuilt from the log.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReplaySummary {
    pub records: u64,
    pub locations: usize,
    pub forks: usize,
    pub changesets: usize,
}

/// The authority's state: the log, its projections and the blobs, in one directory.
///
/// Each call that changes the log is one redb write transaction that runs the event
/// through the transition rules ([`crate::state`]), appends exactly one record, and
/// updates the projections, so after a crash the store holds the record and its
/// effects, or neither. Commits use immediate durability: a call that returned `Ok`
/// has been fsynced.
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
    /// already holds reads nothing and returns its record. Blobs are never removed.
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
        table
            .get(oid.as_bytes().as_slice())?
            .map(|v| decode_blob(oid, v.value()))
            .transpose()
    }

    /// A blob's content, verified against its id on the way out.
    pub fn read_blob(&self, oid: GitOid) -> Result<Vec<u8>> {
        if self.blob_info(oid)?.is_none() {
            return Err(CoreError::MissingBlob(oid));
        }
        self.blobs.read(oid)
    }

    /// Admission: an entry may enter the log only if its blob is ingested and, where a
    /// content rule applies, the blob's bytes pass it. Each blob is read once per mode
    /// and file name, however many paths name it. Checked before the write transaction
    /// opens (blobs are never removed, so the answer cannot change), to keep large
    /// reads out of the single writer's lock. These checks need blob content, so
    /// [`Store::replay_check`] does not repeat them.
    fn check_entries<'a>(
        &self,
        entries: impl IntoIterator<Item = (&'a RepoPath, &'a Entry)>,
    ) -> Result<()> {
        let mut checked = HashSet::new();
        for (path, entry) in entries {
            let oid = entry.oid();
            if self.blob_info(oid)?.is_none() {
                return Err(CoreError::MissingBlob(oid));
            }
            if has_content_rule(path, entry.mode())
                && checked.insert((oid, entry.mode(), path.file_name().to_string()))
            {
                check_content(path, entry.mode(), &self.blobs.read(oid)?)?;
            }
        }
        Ok(())
    }

    // ---- writes -----------------------------------------------------------------

    /// Run `event` through the transition rules against the tables and, unless it is
    /// a retry, append it. `extra` runs inside the same transaction after the rules
    /// pass, for writes the event implies beyond the projections (tree objects).
    fn record(
        &self,
        event: Event,
        extra: impl FnOnce(&WriteTransaction) -> Result<()>,
    ) -> Result<(u64, bool)> {
        let txn = self.begin()?;
        let offset;
        {
            let mut log = txn.open_table(LOG)?;
            offset = last_offset(&log)?.map_or(0, |last| last + 1);
            let mut view = Tables {
                locations: txn.open_table(LOCATIONS)?,
                forks: txn.open_table(FORKS)?,
                changesets: txn.open_table(CHANGESETS)?,
                log: &log,
            };
            if let Applied::Repeat(at) = apply(&mut view, offset, &event)? {
                return Ok((at, false));
            }
            drop(view);
            extra(&txn)?;
            let record = Record {
                offset,
                at_ms: now_ms(),
                event,
            };
            log.insert(offset, to_json(&record).as_slice())?;
        }
        txn.commit()?;
        Ok((offset, true))
    }

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
        self.check_entries(main.entries())?;
        let event = Event::LocationAdded {
            location: name.clone(),
            remote: remote.map(str::to_string),
            main: main.git_tree_id(),
        };
        let (offset, _) =
            self.record(event, |txn| store_tree(&mut txn.open_table(TREES)?, main))?;
        Ok(offset)
    }

    /// Fork `location` at its current main.
    pub fn create_fork(&self, fork: &ForkName, location: &LocationName) -> Result<u64> {
        let main = self
            .location(location)?
            .ok_or_else(|| CoreError::UnknownLocation(location.to_string()))?
            .main;
        let event = Event::ForkCreated {
            fork: fork.clone(),
            location: location.clone(),
            base: main,
        };
        // Main may move between the read above and the transaction; the rules then
        // refuse the fork rather than start it from a stale main.
        Ok(self.record(event, |_| Ok(()))?.0)
    }

    /// Publish a draft: `changeset` must start from the fork's head, every blob it puts
    /// must be ingested, and content with a rule (symlinks, `.gitattributes`) must pass
    /// it. Publishing the changeset that is already the fork's head is a retry: it
    /// appends nothing and returns the first publish's offset. A retry that arrives
    /// after a newer draft is refused as `NotAtBase`, like any stale publish.
    pub fn publish_draft(&self, fork: &ForkName, changeset: &Changeset) -> Result<Published> {
        self.check_entries(changeset.ops.iter().filter_map(|op| match op {
            Op::Put { path, entry } => Some((path, entry)),
            _ => None,
        }))?;
        let id = ChangesetId::of(changeset);
        let event = Event::DraftPublished {
            fork: fork.clone(),
            id,
            changeset: changeset.clone(),
        };
        let (offset, fresh) = self.record(event, |txn| {
            // The rules checked the base by id; applying checks the ops and the target.
            let mut trees = txn.open_table(TREES)?;
            let target = changeset.apply(&load_tree(&trees, changeset.base)?)?;
            store_tree(&mut trees, &target)
        })?;
        Ok(Published { id, offset, fresh })
    }

    /// Record the intent to land `id`, which must be the fork's head. Recording the
    /// same intent again while it is the latest is a retry and appends nothing.
    pub fn record_landing_intent(&self, fork: &ForkName, id: ChangesetId) -> Result<u64> {
        let event = Event::LandingIntent {
            fork: fork.clone(),
            changeset: id,
        };
        Ok(self.record(event, |_| Ok(()))?.0)
    }

    /// Move the main of `fork`'s location by `id`, the fork's head, as a
    /// compare-and-swap: the changeset must start from the main the location has now.
    /// Advancing by the changeset that last moved main is a retry: it appends nothing
    /// and returns that move's offset.
    pub fn advance_main(&self, fork: &ForkName, id: ChangesetId) -> Result<u64> {
        let location = self
            .fork(fork)?
            .ok_or_else(|| CoreError::UnknownFork(fork.to_string()))?
            .location;
        let event = Event::MainAdvanced {
            location,
            fork: fork.clone(),
            changeset: id,
        };
        Ok(self.record(event, |_| Ok(()))?.0)
    }

    // ---- reads ------------------------------------------------------------------

    pub fn location(&self, name: &LocationName) -> Result<Option<Location>> {
        let txn = self.db.begin_read()?;
        get_json(&txn.open_table(LOCATIONS)?, name.as_str())
    }

    pub fn locations(&self) -> Result<Vec<Location>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(LOCATIONS)?;
        table.iter()?.map(|r| from_json(r?.1.value())).collect()
    }

    pub fn fork(&self, name: &ForkName) -> Result<Option<Fork>> {
        let txn = self.db.begin_read()?;
        get_json(&txn.open_table(FORKS)?, name.as_str())
    }

    pub fn forks(&self) -> Result<Vec<Fork>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(FORKS)?;
        table.iter()?.map(|r| from_json(r?.1.value())).collect()
    }

    /// The tree a location's main is at.
    pub fn main_tree(&self, location: &LocationName) -> Result<Tree> {
        let txn = self.db.begin_read()?;
        let loc: Location = get_json(&txn.open_table(LOCATIONS)?, location.as_str())?
            .ok_or_else(|| CoreError::UnknownLocation(location.to_string()))?;
        load_tree(&txn.open_table(TREES)?, loc.main)
    }

    /// The tree a fork's head is at.
    pub fn fork_head(&self, fork: &ForkName) -> Result<Tree> {
        let txn = self.db.begin_read()?;
        let state: Fork = get_json(&txn.open_table(FORKS)?, fork.as_str())?
            .ok_or_else(|| CoreError::UnknownFork(fork.to_string()))?;
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
        indexed_changeset(&txn.open_table(CHANGESETS)?, &txn.open_table(LOG)?, id)
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

    /// Replay the log from empty through the same transition rules the writer uses
    /// (`state.rs`), and compare the result with the projection tables, key by key.
    ///
    /// Fails if an offset is missing or out of place, a record does not decode, an
    /// event breaks a transition rule (or repeats the latest record of its kind, which
    /// only a duplicated record could), or the locations, forks or changeset index
    /// differ from the replay's. The writer's admission checks are not repeated: blob
    /// presence and content rules, a draft's ops and target against its base tree, and
    /// remote syntax need blob or tree content. A record that bypassed the writer can
    /// therefore pass this check and still name a missing blob or tree; reads then
    /// fail loudly ([`Store::tree`] checks a tree against its id, [`Store::read_blob`]
    /// a blob).
    pub fn replay_check(&self) -> Result<ReplaySummary> {
        let txn = self.db.begin_read()?;
        let log = txn.open_table(LOG)?;
        let mut memory = Memory::default();
        let mut records = 0u64;
        for item in log.iter()? {
            let (k, v) = item?;
            let record = decode_record(k.value(), v.value())?;
            let corrupt = |reason: String| CoreError::CorruptLog {
                offset: record.offset,
                reason,
            };
            if record.offset != records {
                return Err(corrupt(format!("expected offset {records}")));
            }
            records += 1;
            match apply(&mut memory, record.offset, &record.event) {
                Ok(Applied::Fresh) => {}
                Ok(Applied::Repeat(at)) => {
                    return Err(corrupt(format!("duplicates the record at {at}")));
                }
                Err(e) => return Err(corrupt(e.to_string())),
            }
        }

        let differ = |what: &str| CoreError::Inconsistent(format!("{what} differ from the log"));
        let locations: BTreeMap<String, Location> = txn
            .open_table(LOCATIONS)?
            .iter()?
            .map(|r| {
                let (k, v) = r?;
                Ok((k.value().to_string(), from_json(v.value())?))
            })
            .collect::<Result<_>>()?;
        let replayed: BTreeMap<String, Location> = memory
            .locations
            .iter()
            .map(|(name, l)| (name.to_string(), l.clone()))
            .collect();
        if locations != replayed {
            return Err(differ("locations"));
        }
        let forks: BTreeMap<String, Fork> = txn
            .open_table(FORKS)?
            .iter()?
            .map(|r| {
                let (k, v) = r?;
                Ok((k.value().to_string(), from_json(v.value())?))
            })
            .collect::<Result<_>>()?;
        let replayed: BTreeMap<String, Fork> = memory
            .forks
            .iter()
            .map(|(name, f)| (name.to_string(), f.clone()))
            .collect();
        if forks != replayed {
            return Err(differ("forks"));
        }
        let index: BTreeMap<Vec<u8>, u64> = txn
            .open_table(CHANGESETS)?
            .iter()?
            .map(|r| r.map(|(k, v)| (k.value().to_vec(), v.value())))
            .collect::<Result<_, _>>()?;
        let replayed: BTreeMap<Vec<u8>, u64> = memory
            .changesets
            .iter()
            .map(|(id, (offset, _))| (id.as_bytes().to_vec(), *offset))
            .collect();
        if index != replayed {
            return Err(differ("changeset index"));
        }
        Ok(ReplaySummary {
            records,
            locations: memory.locations.len(),
            forks: memory.forks.len(),
            changesets: memory.changesets.len(),
        })
    }
}

/// The projection tables of one write transaction, as the rules see them.
struct Tables<'t> {
    locations: redb::Table<'t, &'static str, Bytes>,
    forks: redb::Table<'t, &'static str, Bytes>,
    changesets: redb::Table<'t, Bytes, u64>,
    log: &'t redb::Table<'t, u64, Bytes>,
}

impl View for Tables<'_> {
    fn location(&self, name: &LocationName) -> Result<Option<Location>> {
        get_json(&self.locations, name.as_str())
    }

    fn fork(&self, name: &ForkName) -> Result<Option<Fork>> {
        get_json(&self.forks, name.as_str())
    }

    fn changeset(&self, id: ChangesetId) -> Result<Option<Changeset>> {
        indexed_changeset(&self.changesets, self.log, id)
    }

    fn put_location(&mut self, location: &Location) -> Result<()> {
        self.locations
            .insert(location.name.as_str(), to_json(location).as_slice())?;
        Ok(())
    }

    fn put_fork(&mut self, fork: &Fork) -> Result<()> {
        self.forks
            .insert(fork.name.as_str(), to_json(fork).as_slice())?;
        Ok(())
    }

    fn index_changeset(&mut self, id: ChangesetId, offset: u64, _: &Changeset) -> Result<()> {
        if self.changesets.get(id.as_bytes().as_slice())?.is_none() {
            self.changesets.insert(id.as_bytes().as_slice(), offset)?;
        }
        Ok(())
    }
}

fn indexed_changeset(
    index: &impl ReadableTable<Bytes, u64>,
    log: &impl ReadableTable<u64, Bytes>,
    id: ChangesetId,
) -> Result<Option<Changeset>> {
    let Some(offset) = index.get(id.as_bytes().as_slice())?.map(|v| v.value()) else {
        return Ok(None);
    };
    let value = log.get(offset)?.ok_or(CoreError::CorruptLog {
        offset,
        reason: "the changeset index points past the log".into(),
    })?;
    match decode_record(offset, value.value())?.event {
        Event::DraftPublished { changeset, .. } => Ok(Some(changeset)),
        other => Err(CoreError::CorruptLog {
            offset,
            reason: format!("the changeset index points at {other:?}"),
        }),
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

fn get_json<T: for<'de> Deserialize<'de>>(
    table: &impl ReadableTable<&'static str, Bytes>,
    key: &str,
) -> Result<Option<T>> {
    table.get(key)?.map(|v| from_json(v.value())).transpose()
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

fn decode_blob(oid: GitOid, bytes: &[u8]) -> Result<BlobInfo> {
    let (Some(sha), Some(size)) = (bytes.get(..32), bytes.get(32..40)) else {
        return Err(CoreError::CorruptBlob(oid));
    };
    Ok(BlobInfo {
        sha256: ContentHash::from_bytes(sha.try_into().expect("32 bytes")),
        size: u64::from_le_bytes(size.try_into().expect("8 bytes")),
    })
}

/// One stored git tree object's children: `(mode, name, oid)`.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (LocationName, ForkName) {
        (LocationName::new("l").unwrap(), ForkName::new("f").unwrap())
    }

    /// A location, a fork and one draft on it: four records, offsets 0..3.
    fn setup() -> (tempfile::TempDir, Store, Changeset) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let put = |c: &[u8]| {
            let oid = GitOid::for_blob(c);
            store.ingest_blob(oid, c.len() as u64, c).unwrap();
            Entry::File(oid)
        };
        let main = Tree::from_entries([(RepoPath::new("d/a").unwrap(), put(b"1"))]).unwrap();
        let (l, f) = ids();
        store.add_location(&l, None, &main).unwrap();
        store.create_fork(&f, &l).unwrap();
        let cs = Changeset::new(
            &main,
            vec![Op::put(RepoPath::new("d/a").unwrap(), put(b"2"))],
        )
        .unwrap();
        store.publish_draft(&f, &cs).unwrap();
        store.create_fork(&ForkName::new("g").unwrap(), &l).unwrap();
        assert_eq!(store.replay_check().unwrap().records, 4);
        (dir, store, cs)
    }

    /// Write `record` into the log behind the rules' back.
    fn inject(store: &Store, key: u64, record: Record) {
        let txn = store.begin().unwrap();
        txn.open_table(LOG)
            .unwrap()
            .insert(key, to_json(&record).as_slice())
            .unwrap();
        txn.commit().unwrap();
    }

    fn inject_event(store: &Store, offset: u64, event: Event) {
        inject(
            store,
            offset,
            Record {
                offset,
                at_ms: 0,
                event,
            },
        );
    }

    fn corrupt(store: &Store) -> String {
        match store.replay_check() {
            Err(CoreError::CorruptLog { reason, .. }) => reason,
            other => panic!("expected a corrupt log, got {other:?}"),
        }
    }

    fn inconsistent(store: &Store) -> String {
        match store.replay_check() {
            Err(CoreError::Inconsistent(what)) => what,
            other => panic!("expected inconsistent tables, got {other:?}"),
        }
    }

    #[test]
    fn replay_refuses_a_gap() {
        let (_d, store, _) = setup();
        let (l, _) = ids();
        let g = ForkName::new("h").unwrap();
        let main = store.location(&l).unwrap().unwrap().main;
        inject_event(
            &store,
            5,
            Event::ForkCreated {
                fork: g,
                location: l,
                base: main,
            },
        );
        assert!(corrupt(&store).contains("expected offset 4"));
    }

    #[test]
    fn replay_refuses_a_record_filed_under_another_offset() {
        let (_d, store, _) = setup();
        let (l, _) = ids();
        let main = store.location(&l).unwrap().unwrap().main;
        let event = Event::ForkCreated {
            fork: ForkName::new("h").unwrap(),
            location: l,
            base: main,
        };
        inject(
            &store,
            4,
            Record {
                offset: 7,
                at_ms: 0,
                event,
            },
        );
        assert!(corrupt(&store).contains("record says offset 7"));
    }

    #[test]
    fn replay_refuses_a_duplicated_draft() {
        let (_d, store, cs) = setup();
        let (_, f) = ids();
        let id = ChangesetId::of(&cs);
        inject_event(
            &store,
            4,
            Event::DraftPublished {
                fork: f,
                id,
                changeset: cs,
            },
        );
        assert!(corrupt(&store).contains("duplicates the record at 2"));
    }

    #[test]
    fn replay_refuses_a_draft_under_a_wrong_id() {
        let (_d, store, cs) = setup();
        let (_, f) = ids();
        let back = Changeset::between(&store.fork_head(&f).unwrap(), &store.tree(cs.base).unwrap());
        let id = ChangesetId::of(&cs);
        inject_event(
            &store,
            4,
            Event::DraftPublished {
                fork: f,
                id,
                changeset: back,
            },
        );
        assert!(corrupt(&store).contains("is not its changeset's id"));
    }

    #[test]
    fn replay_refuses_a_fork_that_does_not_start_at_main() {
        let (_d, store, cs) = setup();
        let (l, _) = ids();
        let event = Event::ForkCreated {
            fork: ForkName::new("h").unwrap(),
            location: l,
            base: cs.target,
        };
        inject_event(&store, 4, event);
        assert!(corrupt(&store).contains("starts from"));
    }

    #[test]
    fn replay_refuses_main_moved_by_another_locations_fork() {
        let (_d, store, cs) = setup();
        let (_, f) = ids();
        let other = LocationName::new("m").unwrap();
        let main = store
            .location(&LocationName::new("l").unwrap())
            .unwrap()
            .unwrap()
            .main;
        inject_event(
            &store,
            4,
            Event::LocationAdded {
                location: other.clone(),
                remote: None,
                main,
            },
        );
        let event = Event::MainAdvanced {
            location: other,
            fork: f,
            changeset: ChangesetId::of(&cs),
        };
        inject_event(&store, 5, event);
        assert!(corrupt(&store).contains("is not a fork of location"));
    }

    fn rewrite<T: Serialize + for<'de> Deserialize<'de>>(
        store: &Store,
        table: TableDefinition<&str, Bytes>,
        key: &str,
        change: impl FnOnce(&mut T),
    ) {
        let txn = store.begin().unwrap();
        {
            let mut t = txn.open_table(table).unwrap();
            let mut value: T = from_json(t.get(key).unwrap().unwrap().value()).unwrap();
            change(&mut value);
            t.insert(key, to_json(&value).as_slice()).unwrap();
        }
        txn.commit().unwrap();
    }

    #[test]
    fn replay_refuses_a_locations_table_the_log_does_not_explain() {
        let (_d, store, _) = setup();
        rewrite(&store, LOCATIONS, "l", |l: &mut Location| {
            l.remote = Some("git@example.com:x.git".into())
        });
        assert_eq!(inconsistent(&store), "locations differ from the log");
    }

    #[test]
    fn replay_refuses_a_location_filed_under_another_key() {
        let (_d, store, _) = setup();
        let txn = store.begin().unwrap();
        {
            let mut t = txn.open_table(LOCATIONS).unwrap();
            let row = t.remove("l").unwrap().unwrap().value().to_vec();
            t.insert("alias", row.as_slice()).unwrap();
        }
        txn.commit().unwrap();
        assert_eq!(inconsistent(&store), "locations differ from the log");
    }

    #[test]
    fn a_short_blob_index_row_is_an_error_not_a_panic() {
        let (_d, store, _) = setup();
        let oid = GitOid::for_blob(b"1");
        let txn = store.begin().unwrap();
        txn.open_table(BLOBS)
            .unwrap()
            .insert(oid.as_bytes().as_slice(), &b"short"[..])
            .unwrap();
        txn.commit().unwrap();
        assert!(matches!(store.blob_info(oid), Err(CoreError::CorruptBlob(o)) if o == oid));
    }

    #[test]
    fn replay_refuses_a_forks_table_the_log_does_not_explain() {
        let (_d, store, _) = setup();
        rewrite(&store, FORKS, "f", |f: &mut Fork| f.head = f.base);
        assert_eq!(inconsistent(&store), "forks differ from the log");
    }

    #[test]
    fn replay_refuses_a_changeset_index_the_log_does_not_explain() {
        let (_d, store, cs) = setup();
        let txn = store.begin().unwrap();
        txn.open_table(CHANGESETS)
            .unwrap()
            .insert(ChangesetId::of(&cs).as_bytes().as_slice(), 3)
            .unwrap();
        txn.commit().unwrap();
        assert_eq!(inconsistent(&store), "changeset index differ from the log");
    }

    #[test]
    fn a_stored_tree_that_does_not_rebuild_to_its_id_is_refused() {
        let (_d, store, cs) = setup();
        // Rename the root's only child: the tree now holds e/a, not d/a.
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
