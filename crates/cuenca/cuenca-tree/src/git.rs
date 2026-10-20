//! A bridge to a real git repository through git's plumbing commands.
//!
//! Reading lists a tree with `ls-tree -r -t -z --full-tree` and streams every blob through
//! one `cat-file --batch`, hashing as it streams, so a blob's git id, SHA-256 and size
//! all come from its bytes and a large blob is never held in memory. Writing stores
//! blobs with `hash-object -w --no-filters` and builds each directory with `mktree -z`,
//! so git, not this crate, lays out and sorts the tree objects; any difference from
//! [`Tree::git_objects`] is an error. Every blob a write refers to is verified against
//! its entry's claims, including blobs the repository already has.
//!
//! Scope: SHA-1 repositories with UTF-8 paths. A SHA-256 repository's ids, a non-UTF-8
//! path, a submodule or an empty directory is refused, never approximated.
//!
//! Every command runs with `GIT_NO_REPLACE_OBJECTS=1`, so a `refs/replace/` entry cannot
//! substitute an object; with `GIT_NO_LAZY_FETCH=1`, so a blob missing from a partial
//! clone is reported missing rather than fetched over the network; and without the
//! environment variables that would redirect git to another repository, index or object
//! store.

use crate::entry::{Blob, BlobHasher, Entry, Mode};
use crate::error::TreeError;
use crate::oid::GitOid;
use crate::path::RepoPath;
use crate::tree::Tree;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use thiserror::Error;

/// The content of blobs, by git id.
pub type Blobs = HashMap<GitOid, Vec<u8>>;

/// The largest blob [`Repo::read_blobs`] loads into memory by default.
pub const DEFAULT_MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024;

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
    #[error("object {0} is missing from the repository (a partial clone does not fetch here)")]
    Missing(GitOid),
    #[error("object {0} is not a blob")]
    NotABlob(GitOid),
    #[error("blob {0} does not match its id or its entry's SHA-256 and size")]
    Corrupt(GitOid),
    #[error("blob {oid} is {size} bytes, over the {limit}-byte limit for loading into memory")]
    TooLarge { oid: GitOid, size: u64, limit: u64 },
    #[error("no content was given for blob {0}")]
    NoContent(GitOid),
    #[error("tree object: git built {git}, cuenca computed {ours}")]
    TreeMismatch { git: GitOid, ours: GitOid },
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
}

impl Repo {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
        }
    }

    /// Set the largest blob [`Repo::read_blobs`] will load into memory.
    pub fn with_max_blob_bytes(mut self, limit: u64) -> Self {
        self.max_blob_bytes = limit;
        self
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.path).args(args);
        for var in REDIRECTING_ENV {
            cmd.env_remove(var);
        }
        cmd.env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
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
            return Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        written?;
        Ok(output.stdout)
    }

    /// The tree id `treeish` names.
    pub fn resolve_tree(&self, treeish: &str) -> Result<GitOid, GitError> {
        let spec = format!("{treeish}^{{tree}}");
        let out = self.run(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &spec,
            ],
            b"",
        )?;
        let hex = String::from_utf8_lossy(&out).trim().to_string();
        Ok(GitOid::from_hex(&hex)?)
    }

    /// Read the tree `treeish` names, hashing every blob from its bytes, and check that
    /// the tree id cuenca computes for it is the one git holds.
    pub fn read_tree(&self, treeish: &str) -> Result<Tree, GitError> {
        let root = self.resolve_tree(treeish)?;
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
        // `-t` lists every directory; one with no file under it is an empty tree, which
        // a checkout (and this format) cannot hold.
        let has_files: HashSet<&str> = records
            .iter()
            .flat_map(|(p, _, _)| p.as_str().match_indices('/').map(|(i, _)| &p.as_str()[..i]))
            .collect();
        if let Some(empty) = dirs.iter().find(|d| !has_files.contains(d.as_str())) {
            return Err(TreeError::EmptyDirectory(empty.to_string()).into());
        }

        let wanted = unique(records.iter().map(|r| r.2));
        let mut blobs: HashMap<GitOid, Blob> = HashMap::with_capacity(wanted.len());
        self.cat_file_batch(&wanted, |oid, size, content| {
            blobs.insert(oid, hash_stream(oid, size, content)?);
            Ok(())
        })?;

        let mut entries = Vec::with_capacity(records.len());
        for (path, mode, oid) in records {
            entries.push((path, Entry::from_mode(mode, blobs[&oid])?));
        }
        let tree = Tree::from_entries(entries)?;
        let ours = tree.git_tree_id();
        if ours != root {
            return Err(GitError::TreeMismatch { git: root, ours });
        }
        Ok(tree)
    }

    /// The content of each blob in `oids`, each checked against its id and no larger
    /// than this repository's limit ([`Repo::with_max_blob_bytes`]).
    pub fn read_blobs(&self, oids: impl IntoIterator<Item = GitOid>) -> Result<Blobs, GitError> {
        let wanted = unique(oids);
        let mut out = Blobs::with_capacity(wanted.len());
        let limit = self.max_blob_bytes;
        self.cat_file_batch(&wanted, |oid, size, content| {
            if size > limit {
                return Err(GitError::TooLarge { oid, size, limit });
            }
            let mut bytes = Vec::with_capacity(size as usize);
            content.read_to_end(&mut bytes)?;
            if GitOid::for_blob(&bytes) != oid {
                return Err(GitError::Corrupt(oid));
            }
            out.insert(oid, bytes);
            Ok(())
        })?;
        Ok(out)
    }

    /// The content of every blob `tree` refers to, each checked against its entry.
    pub fn read_tree_blobs(&self, tree: &Tree) -> Result<Blobs, GitError> {
        let blobs = self.read_blobs(tree.entries().map(|(_, e)| e.oid()))?;
        for (_, entry) in tree.entries() {
            if !entry.blob().matches(&blobs[&entry.oid()]) {
                return Err(GitError::Corrupt(entry.oid()));
            }
        }
        Ok(blobs)
    }

    /// Write `tree` into this repository and return its tree id.
    ///
    /// Every blob is verified against its entry by both hashes and the size: blobs the
    /// repository lacks are taken from `blobs`, and blobs it already has are re-read and
    /// hashed. Each directory is built by `git mktree` and must come out as cuenca
    /// computed it.
    pub fn write_tree(&self, tree: &Tree, blobs: &Blobs) -> Result<GitOid, GitError> {
        let needed: Vec<Blob> = {
            let mut seen = HashSet::new();
            tree.entries()
                .map(|(_, e)| *e.blob())
                .filter(|b| seen.insert(b.oid))
                .collect()
        };
        let present = self.present(&needed.iter().map(|b| b.oid).collect::<Vec<_>>())?;

        let (have, lack): (Vec<Blob>, Vec<Blob>) =
            needed.into_iter().partition(|b| present.contains(&b.oid));
        let claims: HashMap<GitOid, Blob> = have.iter().map(|b| (b.oid, *b)).collect();
        self.cat_file_batch(
            &have.iter().map(|b| b.oid).collect::<Vec<_>>(),
            |oid, size, content| {
                if hash_stream(oid, size, content)? != claims[&oid] {
                    return Err(GitError::Corrupt(oid));
                }
                Ok(())
            },
        )?;
        for blob in &lack {
            let content = blobs.get(&blob.oid).ok_or(GitError::NoContent(blob.oid))?;
            if !blob.matches(content) {
                return Err(GitError::Corrupt(blob.oid));
            }
            let out = self.run(&["hash-object", "-w", "--no-filters", "--stdin"], content)?;
            if GitOid::from_hex(String::from_utf8_lossy(&out).trim())? != blob.oid {
                return Err(GitError::Corrupt(blob.oid));
            }
        }

        let mut root = GitOid::empty_tree();
        for object in tree.git_objects() {
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
            root = git;
        }
        Ok(root)
    }

    /// Which of `oids` the repository already has, as blobs.
    fn present(&self, oids: &[GitOid]) -> Result<HashSet<GitOid>, GitError> {
        let input: String = oids.iter().map(|o| format!("{o}\n")).collect();
        let out = self.run(&["cat-file", "--batch-check"], input.as_bytes())?;
        let mut present = HashSet::new();
        for line in String::from_utf8_lossy(&out).lines() {
            match line.split(' ').collect::<Vec<_>>().as_slice() {
                [_, "missing"] => {}
                [oid, "blob", _] => {
                    present.insert(GitOid::from_hex(oid)?);
                }
                [oid, _, _] => return Err(GitError::NotABlob(GitOid::from_hex(oid)?)),
                _ => return Err(GitError::Parse(format!("cat-file --batch-check {line:?}"))),
            }
        }
        Ok(present)
    }

    /// Stream each blob in `oids` through one `git cat-file --batch`, in order. `each`
    /// gets the blob's size and a reader over exactly its content.
    fn cat_file_batch(
        &self,
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
                    [id, "blob", size] if *id == oid.to_hex() => {
                        let size: u64 = size
                            .parse()
                            .map_err(|_| GitError::Parse("cat-file size".into()))?;
                        let mut content = (&mut out).take(size);
                        each(*oid, size, &mut content)?;
                        // Whatever `each` left unread is drained, so the stream stays in step.
                        std::io::copy(&mut content, &mut std::io::sink())?;
                        let mut newline = [0u8; 1];
                        out.read_exact(&mut newline)?;
                    }
                    [id, _, _] if *id == oid.to_hex() => return Err(GitError::NotABlob(*oid)),
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
        let failed = || GitError::Failed {
            args: "cat-file --batch".into(),
            stderr: stderr.trim().to_string(),
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

/// Hash a blob's content as it streams and check it is the object git named.
fn hash_stream(oid: GitOid, size: u64, content: &mut dyn Read) -> Result<Blob, GitError> {
    let mut hasher = BlobHasher::new(size);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = content.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    match hasher.try_finish() {
        Some(blob) if blob.oid == oid => Ok(blob),
        _ => Err(GitError::Corrupt(oid)),
    }
}

fn unique(oids: impl IntoIterator<Item = GitOid>) -> Vec<GitOid> {
    let mut seen = HashSet::new();
    oids.into_iter().filter(|o| seen.insert(*o)).collect()
}
