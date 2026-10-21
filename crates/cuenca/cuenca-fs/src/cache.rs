//! The stat cache: what each file's stat was when its content was last hashed.

use crate::error::{FsError, Result};
use cuenca_tree::Entry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::Metadata;
use std::io::{self, Write as _};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// How much older than the scan a file's modification time must be before its entry is
/// cached. Two seconds covers the coarsest clock a location is likely to sit on (FAT and
/// some SMB servers store two-second times); ext4 and APFS are far finer.
pub const RACY_MARGIN: Duration = Duration::from_secs(2);

const FORMAT: u32 = 1;

/// The parts of a file's `lstat` that change when its content or mode does: device and
/// inode (a replaced file), size, modification and change times to the nanosecond, and
/// the full mode. A content write moves the modification time, and anything that resets
/// it afterwards (`touch -r`, a restored backup) moves the change time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
    mode: u32,
}

impl Stamp {
    pub fn of(meta: &Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
            mode: meta.mode(),
        }
    }

    /// The same stamp from a raw `stat`, as `fstatat`/`fstat` return it. It equals
    /// [`Stamp::of`] for the same file (tested).
    pub(crate) fn from_stat(st: &rustix::fs::Stat) -> Self {
        #[allow(clippy::unnecessary_cast)] // the field types differ between platforms
        Self {
            dev: st.st_dev as u64,
            ino: st.st_ino as u64,
            size: st.st_size as u64,
            mtime: (st.st_mtime as i64, st.st_mtime_nsec as i64),
            ctime: (st.st_ctime as i64, st.st_ctime_nsec as i64),
            mode: st.st_mode as u32,
        }
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// The permission bits, without the file type.
    pub(crate) fn permissions(&self) -> u32 {
        self.mode & 0o7777
    }

    /// Whether the owner's execute bit is set, which is what git reads as executable.
    pub(crate) fn is_executable(&self) -> bool {
        self.mode & 0o100 != 0
    }

    /// Whether the modification time is at least [`RACY_MARGIN`] before `at`, so any
    /// later write gives the file a different stamp.
    pub(crate) fn settled(&self, at: SystemTime) -> bool {
        let (secs, nanos) = self.mtime;
        let Ok(at) = at.duration_since(SystemTime::UNIX_EPOCH) else {
            return false;
        };
        let at = at.saturating_sub(RACY_MARGIN);
        let at_secs = at.as_secs() as i64;
        (secs, nanos) <= (at_secs, i64::from(at.subsec_nanos()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cached {
    pub(crate) stamp: Stamp,
    pub(crate) entry: Entry,
}

/// A checkout's record of which stat produced which entry, keyed by path. It is local
/// state of one checkout, trusted as git trusts its index (see the crate docs), and only
/// ever an optimisation: an empty cache gives the same snapshot, only slower.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatCache {
    format: u32,
    entries: BTreeMap<String, Cached>,
}

impl Default for StatCache {
    fn default() -> Self {
        Self::new()
    }
}

impl StatCache {
    pub fn new() -> Self {
        Self {
            format: FORMAT,
            entries: BTreeMap::new(),
        }
    }

    /// The cache saved at `path`. A missing, unreadable-as-JSON or other-format file
    /// gives an empty cache, which only makes the next snapshot read every file.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice::<Self>(&bytes)
                .ok()
                .filter(|c| c.format == FORMAT)
                .unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(FsError::Io {
                path: path.into(),
                source: e,
            }),
        }
    }

    /// Write the cache to `path` atomically: a crash leaves the old file or the new one.
    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(FsError::io(dir))?;
        let bytes = serde_json::to_vec(self).expect("a stat cache serialises");
        tmp.write_all(&bytes).map_err(FsError::io(tmp.path()))?;
        tmp.as_file().sync_all().map_err(FsError::io(tmp.path()))?;
        tmp.persist(path).map_err(|e| FsError::Io {
            path: path.into(),
            source: e.error,
        })?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The cached entry for `path`, if the file's stamp is still the one recorded.
    pub(crate) fn lookup(&self, path: &str, stamp: &Stamp) -> Option<Entry> {
        self.entries
            .get(path)
            .filter(|c| c.stamp == *stamp)
            .map(|c| c.entry)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &Cached)> {
        self.entries.iter()
    }

    /// Record `entry` for `path`, if the stamp is settled at `at` (see [`RACY_MARGIN`]).
    pub(crate) fn record(&mut self, path: String, stamp: Stamp, entry: Entry, at: SystemTime) {
        if stamp.settled(at) {
            self.entries.insert(path, Cached { stamp, entry });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp_at(secs: i64, nanos: i64) -> Stamp {
        Stamp {
            dev: 1,
            ino: 2,
            size: 3,
            mtime: (secs, nanos),
            ctime: (secs, nanos),
            mode: 0o100644,
        }
    }

    #[test]
    fn only_settled_stamps_are_recorded() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut cache = StatCache::new();
        let entry = Entry::file(b"x");
        cache.record("old".into(), stamp_at(998, 0), entry, at);
        cache.record("racy".into(), stamp_at(998, 1), entry, at);
        cache.record("new".into(), stamp_at(999, 999_999_999), entry, at);
        assert_eq!(cache.lookup("old", &stamp_at(998, 0)), Some(entry));
        assert_eq!(cache.lookup("racy", &stamp_at(998, 1)), None);
        assert_eq!(cache.lookup("new", &stamp_at(999, 999_999_999)), None);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_changed_stamp_misses() {
        let at = SystemTime::now();
        let mut cache = StatCache::new();
        let stamp = stamp_at(1, 0);
        cache.record("f".into(), stamp, Entry::file(b"x"), at);
        let mut ctime_moved = stamp;
        ctime_moved.ctime.1 += 1;
        assert_eq!(cache.lookup("f", &ctime_moved), None);
        let mut chmod = stamp;
        chmod.mode = 0o100755;
        assert_eq!(cache.lookup("f", &chmod), None);
        assert!(cache.lookup("f", &stamp).is_some());
    }

    #[test]
    fn a_raw_stat_and_std_metadata_give_one_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"x").unwrap();
        let raw = rustix::fs::statat(
            rustix::fs::CWD,
            &path,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        let std = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(Stamp::from_stat(&raw), Stamp::of(&std));
    }

    #[test]
    fn a_corrupt_or_foreign_cache_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(StatCache::load(&path).unwrap().is_empty());
        std::fs::write(&path, br#"{"format":99,"entries":{}}"#).unwrap();
        assert!(StatCache::load(&path).unwrap().is_empty());
        let mut cache = StatCache::new();
        cache.record(
            "a".into(),
            stamp_at(1, 0),
            Entry::file(b"a"),
            SystemTime::now(),
        );
        cache.save(&path).unwrap();
        assert_eq!(StatCache::load(&path).unwrap(), cache);
    }
}
