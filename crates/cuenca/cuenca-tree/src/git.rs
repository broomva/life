//! A bridge to a real git repository through git's plumbing commands.
//!
//! Reading lists a tree with `ls-tree -r -z --full-tree` and streams every blob through
//! one `cat-file --batch`, re-hashing each so a blob's git id and SHA-256 both come from
//! its bytes. Writing stores blobs with `hash-object -w --no-filters` and builds each
//! directory with `mktree -z`, so git, not this crate, lays out and sorts the tree
//! objects; any difference from [`Tree::git_objects`] is an error.
//!
//! Every command runs with `GIT_NO_REPLACE_OBJECTS=1`, so a `refs/replace/` entry cannot
//! substitute an object; with `GIT_NO_LAZY_FETCH=1`, so a blob missing from a partial
//! clone is reported missing rather than fetched over the network; and without the
//! environment variables that would redirect git to another repository, index or object
//! store.

use crate::entry::{Blob, Entry, Mode};
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

#[derive(Debug, Error)]
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
    #[error("object {0} does not hash to its id")]
    Corrupt(GitOid),
    #[error("no content was given for blob {0}")]
    NoContent(GitOid),
    #[error("tree {dir:?}: git has {git}, cuenca computed {ours}")]
    TreeMismatch {
        dir: String,
        git: GitOid,
        ours: GitOid,
    },
}

/// Environment variables that would point git at another repository or object store.
const REDIRECTING_ENV: &[&str] = &[
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
}

impl Repo {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
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
        writer.join().expect("stdin writer does not panic")?;
        if !output.status.success() {
            return Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
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

    /// Read the tree `treeish` names, re-hashing every blob, and check that the tree id
    /// cuenca computes for it is the one git holds.
    pub fn read_tree(&self, treeish: &str) -> Result<Tree, GitError> {
        let root = self.resolve_tree(treeish)?;
        let listing = self.run(&["ls-tree", "-r", "-z", "--full-tree", &root.to_hex()], b"")?;

        let mut records = Vec::new();
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
            records.push((path, Mode::parse(mode)?, GitOid::from_hex(oid)?));
        }

        let wanted: Vec<GitOid> =
            unique(records.iter().filter(|r| r.1 != Mode::Gitlink).map(|r| r.2));
        let mut blobs: HashMap<GitOid, Blob> = HashMap::with_capacity(wanted.len());
        self.cat_file_batch(&wanted, |oid, content| {
            let blob = Blob::of(content);
            if blob.oid != oid {
                return Err(GitError::Corrupt(oid));
            }
            blobs.insert(oid, blob);
            Ok(())
        })?;

        let mut entries = Vec::with_capacity(records.len());
        for (path, mode, oid) in records {
            let entry = match mode {
                Mode::Gitlink => Entry::Gitlink(oid),
                mode => Entry::from_mode(mode, blobs[&oid])?,
            };
            entries.push((path, entry));
        }
        let tree = Tree::from_entries(entries)?;
        let ours = tree.git_tree_id();
        if ours != root {
            return Err(GitError::TreeMismatch {
                dir: String::new(),
                git: root,
                ours,
            });
        }
        Ok(tree)
    }

    /// The content of each blob in `oids`.
    pub fn read_blobs(&self, oids: impl IntoIterator<Item = GitOid>) -> Result<Blobs, GitError> {
        let wanted = unique(oids);
        let mut out = Blobs::with_capacity(wanted.len());
        self.cat_file_batch(&wanted, |oid, content| {
            out.insert(oid, content.to_vec());
            Ok(())
        })?;
        Ok(out)
    }

    /// The content of every blob `tree` refers to.
    pub fn read_tree_blobs(&self, tree: &Tree) -> Result<Blobs, GitError> {
        self.read_blobs(tree.entries().filter_map(|(_, e)| e.blob()).map(|b| b.oid))
    }

    /// Write `tree` into this repository and return its tree id. Blobs the repository
    /// lacks are taken from `blobs` and must match their entry by both hashes. Each
    /// directory is built by `git mktree`, and must come out as cuenca computed it.
    pub fn write_tree(&self, tree: &Tree, blobs: &Blobs) -> Result<GitOid, GitError> {
        let needed: Vec<Blob> = {
            let mut seen = HashSet::new();
            tree.entries()
                .filter_map(|(_, e)| e.blob().copied())
                .filter(|b| seen.insert(b.oid))
                .collect()
        };
        let present = self.present(&needed.iter().map(|b| b.oid).collect::<Vec<_>>())?;
        for blob in needed.iter().filter(|b| !present.contains(&b.oid)) {
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
                    dir: object.dir,
                    git,
                    ours: object.oid,
                });
            }
            root = git;
        }
        Ok(root)
    }

    /// Which of `oids` the repository already has.
    fn present(&self, oids: &[GitOid]) -> Result<HashSet<GitOid>, GitError> {
        let input: String = oids.iter().map(|o| format!("{o}\n")).collect();
        let out = self.run(&["cat-file", "--batch-check"], input.as_bytes())?;
        let mut present = HashSet::new();
        for line in String::from_utf8_lossy(&out).lines() {
            let mut fields = line.split(' ');
            if let (Some(oid), Some(kind)) = (fields.next(), fields.next())
                && kind != "missing"
            {
                present.insert(GitOid::from_hex(oid)?);
            }
        }
        Ok(present)
    }

    /// Stream each blob in `oids` through one `git cat-file --batch`, in order.
    fn cat_file_batch(
        &self,
        oids: &[GitOid],
        mut each: impl FnMut(GitOid, &[u8]) -> Result<(), GitError>,
    ) -> Result<(), GitError> {
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
        let mut content = Vec::new();
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
                        let size: usize = size
                            .parse()
                            .map_err(|_| GitError::Parse("cat-file size".into()))?;
                        content.resize(size, 0);
                        out.read_exact(&mut content)?;
                        let mut newline = [0u8; 1];
                        out.read_exact(&mut newline)?;
                        each(*oid, &content)?;
                    }
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
        written?;
        if !status.success() {
            return Err(failed());
        }
        Ok(())
    }
}

fn unique(oids: impl IntoIterator<Item = GitOid>) -> Vec<GitOid> {
    let mut seen = HashSet::new();
    oids.into_iter().filter(|o| seen.insert(*o)).collect()
}
