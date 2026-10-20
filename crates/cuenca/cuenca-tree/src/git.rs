//! A bridge to a real git repository through git's plumbing commands.
//!
//! Reading lists a tree with `ls-tree -r -t -z --full-tree` and streams every blob through
//! one `cat-file --batch`, hashing as it streams (with SHA-1 collision detection), so
//! each blob is checked against the id git filed it under and a large blob is never held
//! in memory. Writing checks every blob it was sent before writing anything, stores the
//! missing ones with `hash-object -w --no-filters`, builds each missing directory with
//! `mktree -z` (so git, not this crate, lays out and sorts the tree objects), and then
//! reads the result back, so nothing already in the store is trusted unread. Every tree
//! object read is compared byte for byte with the canonical body for its id, since git's
//! listing normalises modes, order and names and git does not re-hash subtrees. Content
//! rules a tree of ids cannot see ([`check_content`]) are applied on both paths, once
//! per blob id.
//!
//! Needs git 2.25 or later (`rev-parse --show-object-format`).
//!
//! Scope: SHA-1 repositories with UTF-8 paths and no alternates. A SHA-256 repository
//! or one that borrows objects from another store (`objects/info/alternates` or
//! `http-alternates`) is refused by every reading and writing method; a non-UTF-8 path,
//! a submodule or an empty directory is refused on read; writing into a partial clone
//! is refused.
//!
//! Every command runs with `GIT_NO_REPLACE_OBJECTS=1`, so a `refs/replace/` entry cannot
//! substitute an object; with `GIT_NO_LAZY_FETCH=1`, so a blob missing from a partial
//! clone is reported missing rather than fetched over the network; with `LC_ALL=C`, so
//! git's messages can be read; and without the environment variables that would
//! redirect git to another repository, index or object store.
//!
//! Trust boundary: every object a read returns was hashed from the bytes git served to
//! that read, and a write is read back. What no reader can rule out is a store that
//! someone writes into directly: two copies of one id in two packs can be served
//! differently to different git processes. So the store must be written only by git's
//! own hashing writes (`hash-object`, `mktree`, `index-pack --strict`, `fetch`), and
//! untrusted content enters only through [`Repo::write_tree`] or a strict `index-pack`
//! into a store the authority owns (spec section 2a). Reading an agent's own store
//! tells the reader what that agent's git serves, nothing more.

use crate::entry::{Blob, BlobHasher, Entry, HashFailure, Mode};
use crate::error::{SymlinkProblem, TreeError};
use crate::oid::GitOid;
use crate::path::RepoPath;
use crate::tree::{MAX_GITATTRIBUTES_BYTES, MAX_SYMLINK_TARGET, Tree, check_content};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use thiserror::Error;

/// The content of blobs, by git id.
pub type Blobs = HashMap<GitOid, Vec<u8>>;

/// The largest blob [`Repo::read_blobs`] loads into memory by default.
pub const DEFAULT_MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024;
/// The most [`Repo::read_blobs`] loads into memory in one call by default.
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GitError {
    #[error("could not run git: {0}")]
    Io(#[from] std::io::Error),
    #[error("git {args} failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("unexpected output from git {0}")]
    Parse(String),
    #[error("git {0} stopped before answering")]
    EndedEarly(String),
    #[error(transparent)]
    Tree(#[from] TreeError),
    #[error("the repository uses the {0} object format; only sha1 is supported")]
    UnsupportedObjectFormat(String),
    #[error("the repository is a partial clone, which this bridge does not write into")]
    PartialClone,
    #[error("object {0} is missing from the repository (a partial clone does not fetch here)")]
    Missing(GitOid),
    #[error("object {0} is not of the type its entry says")]
    WrongType(GitOid),
    #[error("blob {0} does not hash to its id")]
    Corrupt(GitOid),
    #[error("blob {0} trips SHA-1 collision detection, which git refuses too")]
    Collision(GitOid),
    #[error("blob {oid} is {size} bytes, over the {limit}-byte limit for loading into memory")]
    TooLarge { oid: GitOid, size: u64, limit: u64 },
    #[error("these blobs total over the {limit}-byte limit for loading into memory")]
    TotalTooLarge { limit: u64 },
    #[error("no content was given for blob {0}")]
    NoContent(GitOid),
    #[error("tree object: git built {git}, cuenca computed {ours}")]
    TreeMismatch { git: GitOid, ours: GitOid },
    #[error(
        "the object stored under {0} is not the canonical tree with that id (a corrupt or substituted object)"
    )]
    StoreMismatch(GitOid),
    #[error(
        "tree {stored} rebuilds as {ours}: it is stored in a form cuenca does not produce (a legacy mode or other non-canonical encoding), or substituted"
    )]
    NonCanonical { stored: GitOid, ours: GitOid },
    #[error(
        "the repository borrows objects from another store (objects/info/alternates or http-alternates), which this bridge does not trust"
    )]
    Alternates,
}

/// Environment variables that would point git at another repository or object store.
pub const REDIRECTING_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// A git repository, addressed by a path inside it (a working tree or a bare repository).
#[derive(Debug, Clone)]
pub struct Repo {
    path: PathBuf,
    max_blob_bytes: u64,
    max_total_bytes: u64,
}

impl Repo {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }

    /// Set the largest single blob, and the most in total, that [`Repo::read_blobs`]
    /// loads into memory.
    pub fn with_memory_limits(mut self, per_blob: u64, total: u64) -> Self {
        self.max_blob_bytes = per_blob;
        self.max_total_bytes = total;
        self
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.path).args(args);
        for var in REDIRECTING_ENV.iter().chain(&["LANGUAGE"]) {
            cmd.env_remove(var);
        }
        cmd.env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        cmd
    }

    fn run(&self, args: &[&str], stdin: &[u8]) -> Result<Vec<u8>, GitError> {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut input = child.stdin.take().expect("stdin is piped");
        let stdin = stdin.to_vec();
        let writer = std::thread::spawn(move || input.write_all(&stdin));
        let output = child.wait_with_output()?;
        let written = writer.join().expect("stdin writer does not panic");
        // git's own failure explains a broken pipe on its stdin, so it is reported first.
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(match promisor_miss(&stderr) {
                Some(oid) => GitError::Missing(oid),
                None => GitError::Failed {
                    args: args.join(" "),
                    stderr,
                },
            });
        }
        written?;
        Ok(output.stdout)
    }

    /// Refuse a repository this bridge cannot vouch for: one whose object ids are not
    /// SHA-1, or one that borrows objects from another store. Within a store, every read
    /// is checked against the id it asked for; a second store could hold a different
    /// copy under the same id, and which copy git serves depends on the process.
    fn check_repository(&self) -> Result<(), GitError> {
        let out = self.run(&["rev-parse", "--show-object-format"], b"")?;
        match String::from_utf8_lossy(&out).trim() {
            "sha1" => {}
            other => return Err(GitError::UnsupportedObjectFormat(other.to_string())),
        }
        // The common git directory, from raw bytes (a non-UTF-8 path must not turn into
        // a path that does not exist), relative to the repository path unless absolute.
        // `--git-common-dir` predates every git this bridge supports, unlike
        // `--path-format`.
        let out = self.run(&["rev-parse", "--git-common-dir"], b"")?;
        let line = out.strip_suffix(b"\n").unwrap_or(&out);
        if line.is_empty() || line.contains(&b'\n') {
            return Err(GitError::Parse("rev-parse --git-common-dir".into()));
        }
        let common = std::path::Path::new(std::ffi::OsStr::from_bytes(line));
        let common = if common.is_absolute() {
            common.to_path_buf()
        } else {
            self.path.join(common)
        };
        for name in ["objects/info/alternates", "objects/info/http-alternates"] {
            // Any content at all is refused: git honours a line of only spaces as a
            // relative path, and only a missing file proves there is no second store.
            match std::fs::read(common.join(name)) {
                Ok(bytes) if bytes.is_empty() => {}
                Ok(_) => return Err(GitError::Alternates),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(GitError::Io(e)),
            }
        }
        Ok(())
    }

    /// The tree id `treeish` names.
    pub fn resolve_tree(&self, treeish: &str) -> Result<GitOid, GitError> {
        self.check_repository()?;
        self.resolve_tree_checked(treeish)
    }

    fn resolve_tree_checked(&self, treeish: &str) -> Result<GitOid, GitError> {
        let spec = format!("{treeish}^{{tree}}");
        let out = self.run(&["rev-parse", "--verify", "--end-of-options", &spec], b"")?;
        let hex = String::from_utf8_lossy(&out).trim().to_string();
        Ok(GitOid::from_hex(&hex)?)
    }

    /// Read the tree `treeish` names, hashing every blob from its bytes, and check that
    /// the tree id cuenca computes for it is the one git holds.
    pub fn read_tree(&self, treeish: &str) -> Result<Tree, GitError> {
        self.check_repository()?;
        let root = self.resolve_tree_checked(treeish)?;
        let listing = self.run(
            &["ls-tree", "-r", "-t", "-z", "--full-tree", &root.to_hex()],
            b"",
        )?;

        let mut records = Vec::new();
        let mut dirs = Vec::new();
        for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .ok_or_else(|| GitError::Parse("ls-tree".into()))?;
            let meta = std::str::from_utf8(&record[..tab])
                .map_err(|_| GitError::Parse("ls-tree".into()))?;
            let mut fields = meta.split(' ');
            let (Some(mode), Some(_kind), Some(oid), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(GitError::Parse(format!("ls-tree record {meta:?}")));
            };
            let path = RepoPath::from_bytes(&record[tab + 1..]).map_err(TreeError::from)?;
            match Mode::parse(mode)? {
                Mode::Tree => dirs.push(path),
                mode => records.push((path, mode, GitOid::from_hex(oid)?)),
            }
        }
        // `-t` lists every directory. One that is nobody's parent is an empty tree, which
        // a checkout (and this format) cannot hold.
        let parents: HashSet<&str> = records
            .iter()
            .map(|r| &r.0)
            .chain(&dirs)
            .map(|p| p.split_last().0)
            .collect();
        if let Some(empty) = dirs.iter().find(|d| !parents.contains(d.as_str())) {
            return Err(TreeError::EmptyDirectory(empty.to_string()).into());
        }

        // A blob with a content rule is loaded and checked once per kind of rule,
        // whatever the number of paths naming it.
        let mut ruled: HashMap<GitOid, Ruled> = HashMap::new();
        for (path, mode, oid) in &records {
            ruled_add(&mut ruled, path, &Entry::from_mode(*mode, *oid)?);
        }
        let wanted = unique(records.iter().map(|r| r.2));
        self.cat_file_batch("blob", &wanted, |oid, size, content| {
            match ruled.get(&oid) {
                None => hash_stream(oid, size, content),
                Some(rules) => rules.check_stream(oid, size, content),
            }
        })?;

        let mut entries = Vec::with_capacity(records.len());
        for (path, mode, oid) in records {
            entries.push((path, Entry::from_mode(mode, oid)?));
        }
        let tree = Tree::from_entries(entries)?;
        let ours = tree.git_tree_id();
        if ours != root {
            return Err(GitError::NonCanonical { stored: root, ours });
        }
        // The listing alone can hide a substituted tree object: git prints modes in
        // canonical form, entries in stored order and a name containing `/` like a
        // directory, and it does not re-hash subtrees it parses. So every tree object is
        // read and must be, byte for byte, the canonical body cuenca computed for its id.
        let objects = tree.git_objects();
        let bodies: HashMap<GitOid, Vec<u8>> = objects.iter().map(|o| (o.oid, o.body())).collect();
        let ids = unique(objects.iter().map(|o| o.oid));
        self.cat_file_batch("tree", &ids, |oid, size, content| {
            let expected = &bodies[&oid];
            if size != expected.len() as u64 {
                return Err(GitError::StoreMismatch(oid));
            }
            let mut stored = Vec::with_capacity(expected.len());
            content.read_to_end(&mut stored)?;
            if stored != *expected {
                return Err(GitError::StoreMismatch(oid));
            }
            Ok(())
        })?;
        Ok(tree)
    }

    /// The content of each blob in `oids`, each checked against its id, within this
    /// repository's memory limits ([`Repo::with_memory_limits`]).
    pub fn read_blobs(&self, oids: impl IntoIterator<Item = GitOid>) -> Result<Blobs, GitError> {
        self.check_repository()?;
        let wanted = unique(oids);
        let mut out = Blobs::with_capacity(wanted.len());
        let (per_blob, total) = (self.max_blob_bytes, self.max_total_bytes);
        let mut loaded = 0u64;
        self.cat_file_batch("blob", &wanted, |oid, size, content| {
            if size > per_blob {
                return Err(GitError::TooLarge {
                    oid,
                    size,
                    limit: per_blob,
                });
            }
            loaded += size;
            if loaded > total {
                return Err(GitError::TotalTooLarge { limit: total });
            }
            out.insert(oid, read_checked(oid, size, content, per_blob)?);
            Ok(())
        })?;
        Ok(out)
    }

    /// The content of every blob `tree` refers to, each checked against its id.
    pub fn read_tree_blobs(&self, tree: &Tree) -> Result<Blobs, GitError> {
        self.read_blobs(tree.entries().map(|(_, e)| e.oid()))
    }

    /// Write `tree` into this repository and return its tree id.
    ///
    /// Before anything is written, every blob of the tree the caller sent must hash to its
    /// id and pass its content rules ([`check_content`]), and every blob the repository lacks
    /// must have been sent. Missing blobs are stored with `hash-object`, and each missing
    /// directory is built by `git mktree`, which must lay it out as cuenca computed it.
    /// Finally the result is read back with [`Repo::read_tree`], which re-hashes every
    /// blob, byte-checks every tree object against its canonical body and applies every
    /// rule, so an object in the store that is not what its id names fails the write
    /// rather than landing. Nothing is written into a SHA-256 repository, a partial clone
    /// or a repository with alternates.
    ///
    /// Trust boundary: the store itself must be written only by git's own hashing
    /// writes (`hash-object`, `mktree`, `index-pack --strict`, `fetch`). A writer that
    /// can place objects in the store directly can keep two different copies of one id
    /// in two packs, and which one a given git process serves is not something any reader
    /// can control. Untrusted content enters only through this method or a strict
    /// `index-pack` into a store the authority owns (spec section 2a).
    pub fn write_tree(&self, tree: &Tree, blobs: &Blobs) -> Result<GitOid, GitError> {
        self.check_repository()?;
        let promisor = self
            .run(
                &[
                    "config",
                    "--get-regexp",
                    "^(extensions\\.partialclone|remote\\..*\\.promisor)$",
                ],
                b"",
            )
            .unwrap_or_default();
        if !promisor.is_empty() {
            return Err(GitError::PartialClone);
        }

        let mut ruled: HashMap<GitOid, Ruled> = HashMap::new();
        for (path, entry) in tree.entries() {
            ruled_add(&mut ruled, path, entry);
        }
        let blob_oids = unique(tree.entries().map(|(_, e)| e.oid()));
        for oid in &blob_oids {
            if let Some(content) = blobs.get(oid) {
                verify_bytes(*oid, content)?;
                if let Some(rules) = ruled.get(oid) {
                    rules.check(content)?;
                }
            }
        }

        let objects = tree.git_objects();
        let tree_oids: Vec<GitOid> = objects.iter().map(|t| t.oid).collect();
        let present = self.present(
            &blob_oids
                .iter()
                .chain(&tree_oids)
                .copied()
                .collect::<Vec<_>>(),
        )?;
        for oid in &blob_oids {
            match present.get(oid).map(String::as_str) {
                Some("blob") => {}
                Some(_) => return Err(GitError::WrongType(*oid)),
                None if !blobs.contains_key(oid) => return Err(GitError::NoContent(*oid)),
                None => {}
            }
        }

        // Content rules for blobs the store already has and the caller did not send are
        // checked here too, so a refused write stores nothing.
        let unsent: Vec<GitOid> = ruled
            .keys()
            .filter(|o| present.contains_key(*o) && !blobs.contains_key(*o))
            .copied()
            .collect();
        self.cat_file_batch("blob", &unsent, |oid, size, content| {
            ruled[&oid].check_stream(oid, size, content)
        })?;

        for oid in blob_oids.iter().filter(|o| !present.contains_key(o)) {
            let out = self.run(
                &["hash-object", "-w", "--no-filters", "--stdin"],
                &blobs[oid],
            )?;
            if GitOid::from_hex(String::from_utf8_lossy(&out).trim())? != *oid {
                return Err(GitError::Corrupt(*oid));
            }
        }
        for object in &objects {
            match present.get(&object.oid).map(String::as_str) {
                Some("tree") => continue,
                Some(_) => return Err(GitError::WrongType(object.oid)),
                None => {}
            }
            let mut input = Vec::new();
            for (mode, name, oid) in &object.children {
                input.extend_from_slice(
                    format!("{} {} {oid}\t", mode.as_str(), mode.object_type()).as_bytes(),
                );
                input.extend_from_slice(name.as_bytes());
                input.push(0);
            }
            let out = self.run(&["mktree", "-z"], &input)?;
            let git = GitOid::from_hex(String::from_utf8_lossy(&out).trim())?;
            if git != object.oid {
                return Err(GitError::TreeMismatch {
                    git,
                    ours: object.oid,
                });
            }
        }

        // The read-back re-hashes every blob and byte-checks every tree object against the
        // canonical body for its id, so it returns this tree or an error.
        let root = tree.git_tree_id();
        self.read_tree(&root.to_hex())?;
        Ok(root)
    }

    /// The type of each of `oids` the repository already has.
    fn present(&self, oids: &[GitOid]) -> Result<HashMap<GitOid, String>, GitError> {
        let input: String = oids.iter().map(|o| format!("{o}\n")).collect();
        let out = self.run(&["cat-file", "--batch-check"], input.as_bytes())?;
        let mut present = HashMap::new();
        for line in String::from_utf8_lossy(&out).lines() {
            match line.split(' ').collect::<Vec<_>>().as_slice() {
                [_, "missing"] => {}
                [oid, kind, _] => {
                    present.insert(GitOid::from_hex(oid)?, kind.to_string());
                }
                _ => return Err(GitError::Parse(format!("cat-file --batch-check {line:?}"))),
            }
        }
        Ok(present)
    }

    /// Stream each object in `oids`, all of type `kind`, through one
    /// `git cat-file --batch`, in order. `each` gets the object's size and a reader over
    /// exactly its content.
    fn cat_file_batch(
        &self,
        kind: &str,
        oids: &[GitOid],
        mut each: impl FnMut(GitOid, u64, &mut dyn Read) -> Result<(), GitError>,
    ) -> Result<(), GitError> {
        if oids.is_empty() {
            return Ok(());
        }
        let mut child = self
            .command(&["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut input = child.stdin.take().expect("stdin is piped");
        let request: String = oids.iter().map(|o| format!("{o}\n")).collect();
        let writer = std::thread::spawn(move || input.write_all(request.as_bytes()));
        let mut errors = child.stderr.take().expect("stderr is piped");
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = errors.read_to_string(&mut text);
            text
        });
        let mut out = BufReader::new(child.stdout.take().expect("stdout is piped"));

        let mut header = String::new();
        let result = (|| {
            for oid in oids {
                header.clear();
                if out.read_line(&mut header)? == 0 {
                    return Err(GitError::EndedEarly("cat-file --batch".into()));
                }
                let fields: Vec<&str> = header.trim_end().split(' ').collect();
                match fields.as_slice() {
                    [id, "missing"] if *id == oid.to_hex() => return Err(GitError::Missing(*oid)),
                    [id, found, size] if *id == oid.to_hex() && *found == kind => {
                        let size: u64 = size
                            .parse()
                            .map_err(|_| GitError::Parse("cat-file size".into()))?;
                        let mut content = (&mut out).take(size);
                        each(*oid, size, &mut content)?;
                        // Whatever `each` left unread is drained, so the stream stays in step.
                        std::io::copy(&mut content, &mut std::io::sink())?;
                        let mut newline = [0u8; 1];
                        if out.read(&mut newline)? == 0 {
                            return Err(GitError::EndedEarly("cat-file --batch".into()));
                        }
                        if newline[0] != b'\n' {
                            return Err(GitError::Parse("cat-file separator".into()));
                        }
                    }
                    [id, _, _] if *id == oid.to_hex() => return Err(GitError::WrongType(*oid)),
                    _ => return Err(GitError::Parse(format!("cat-file header {header:?}"))),
                }
            }
            Ok(())
        })();

        // On an early error git may die of a closed pipe, failing the writer too, so the
        // reader's error is the one reported, unless git itself stopped first, in which
        // case git's own message says why.
        drop(out);
        let status = child.wait()?;
        let written = writer.join().expect("stdin writer does not panic");
        let stderr = stderr.join().expect("stderr reader does not panic");
        let failed = || match promisor_miss(&stderr) {
            Some(oid) => GitError::Missing(oid),
            None => GitError::Failed {
                args: "cat-file --batch".into(),
                stderr: stderr.trim().to_string(),
            },
        };
        match result {
            Err(GitError::EndedEarly(_)) if !status.success() => return Err(failed()),
            Err(e) => return Err(e),
            Ok(()) => {}
        }
        if !status.success() {
            return Err(failed());
        }
        written?;
        Ok(())
    }
}

/// The object a partial clone refused to fetch, from git's "could not fetch <oid>"
/// (`LC_ALL=C` keeps it in English). Before git 2.44, cat-file with lazy fetching
/// disabled dies this way instead of answering "<oid> missing"; either way nothing was
/// fetched.
fn promisor_miss(stderr: &str) -> Option<GitOid> {
    let rest = stderr.split("could not fetch ").nth(1)?;
    GitOid::from_hex(rest.get(..40)?).ok()
}

/// Hash a blob's content as it streams and check it is the object git named. A stream
/// that ends short means git stopped, not that the object is corrupt.
fn hash_stream(oid: GitOid, size: u64, content: &mut dyn Read) -> Result<(), GitError> {
    let mut hasher = BlobHasher::new(size);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = content.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    match hasher.finish() {
        Ok(blob) if blob.oid() == oid => Ok(()),
        Ok(_) => Err(GitError::Corrupt(oid)),
        Err(HashFailure::WrongLength { expected, seen }) if seen < expected => {
            Err(GitError::EndedEarly("cat-file --batch".into()))
        }
        Err(HashFailure::WrongLength { .. }) => Err(GitError::Corrupt(oid)),
        Err(HashFailure::Collision) => Err(GitError::Collision(oid)),
    }
}

/// Check that `content` is the blob `oid` names, with git's collision detection.
fn verify_bytes(oid: GitOid, content: &[u8]) -> Result<(), GitError> {
    match Blob::of(content) {
        Some(blob) if blob.oid() == oid => Ok(()),
        Some(_) => Err(GitError::Corrupt(oid)),
        None => Err(GitError::Collision(oid)),
    }
}

/// Read a blob's content into memory (refusing one over `limit`) and check its id.
fn read_checked(
    oid: GitOid,
    size: u64,
    content: &mut dyn Read,
    limit: u64,
) -> Result<Vec<u8>, GitError> {
    if size > limit {
        return Err(GitError::TooLarge { oid, size, limit });
    }
    let mut bytes = Vec::with_capacity(size as usize);
    content.read_to_end(&mut bytes)?;
    if (bytes.len() as u64) < size {
        return Err(GitError::EndedEarly("cat-file --batch".into()));
    }
    verify_bytes(oid, &bytes)?;
    Ok(bytes)
}

/// The content rules that apply to one blob id: the first path naming it as a
/// symlink and the first naming it as a `.gitattributes`.
#[derive(Default)]
struct Ruled<'a> {
    link: Option<&'a RepoPath>,
    attrs: Option<&'a RepoPath>,
}

fn ruled_add<'a>(ruled: &mut HashMap<GitOid, Ruled<'a>>, path: &'a RepoPath, entry: &Entry) {
    if !crate::tree::has_content_rule(path, entry.mode()) {
        return;
    }
    let is_link = entry.mode() == Mode::Symlink;
    let is_attrs = crate::path::is_gitattributes_alias(path.file_name());
    let rules = ruled.entry(entry.oid()).or_default();
    if is_link && rules.link.is_none() {
        rules.link = Some(path);
    }
    if is_attrs && rules.attrs.is_none() {
        rules.attrs = Some(path);
    }
}

impl Ruled<'_> {
    fn check(&self, content: &[u8]) -> Result<(), TreeError> {
        if let Some(path) = self.link {
            check_content(path, Mode::Symlink, content)?;
        }
        if let Some(path) = self.attrs {
            check_content(path, Mode::File, content)?;
        }
        Ok(())
    }

    /// Check a blob as it streams from git, refusing by size before loading it.
    fn check_stream(&self, oid: GitOid, size: u64, content: &mut dyn Read) -> Result<(), GitError> {
        if let Some(path) = self.link
            && size > MAX_SYMLINK_TARGET as u64
        {
            return Err(TreeError::SymlinkTarget {
                path: path.to_string(),
                problem: SymlinkProblem::TooLong(size as usize),
            }
            .into());
        }
        if let Some(path) = self.attrs
            && size > MAX_GITATTRIBUTES_BYTES as u64
        {
            return Err(TreeError::GitattributesContent {
                path: path.to_string(),
                why: format!("{size} bytes, over {MAX_GITATTRIBUTES_BYTES}"),
            }
            .into());
        }
        let bytes = read_checked(oid, size, content, MAX_GITATTRIBUTES_BYTES as u64)?;
        Ok(self.check(&bytes)?)
    }
}

fn unique(oids: impl IntoIterator<Item = GitOid>) -> Vec<GitOid> {
    let mut seen = HashSet::new();
    oids.into_iter().filter(|o| seen.insert(*o)).collect()
}
