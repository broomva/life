//! A scratch repository whose working directory is driven through the filesystem, with
//! a model of what it should hold. Git, reading the working directory, is the oracle.

#![allow(dead_code)]

use cuenca_tree::git::Blobs;
use cuenca_tree::{Entry, GitOid, RepoPath, Tree};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Exec,
    Link,
}

pub struct Work {
    dir: tempfile::TempDir,
    pub model: BTreeMap<String, (Kind, Vec<u8>)>,
}

/// Run git in `dir` with no system or global configuration, so the result does not
/// depend on the machine running the test.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join(".no-xdg"))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_NAMESPACE")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .trim()
        .to_string()
}

impl Work {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        Self {
            dir,
            model: BTreeMap::new(),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    fn abs(&self, path: &str) -> PathBuf {
        self.dir.path().join(path)
    }

    pub fn write(&mut self, path: &str, kind: Kind, content: &[u8]) {
        let abs = self.abs(path);
        fs::create_dir_all(abs.parent().expect("has a parent")).expect("mkdir");
        if fs::symlink_metadata(&abs).is_ok() {
            fs::remove_file(&abs).expect("replace");
        }
        match kind {
            Kind::Link => symlink(OsStr::from_bytes(content), &abs).expect("symlink"),
            Kind::File | Kind::Exec => {
                fs::write(&abs, content).expect("write");
                let mode = if kind == Kind::Exec { 0o755 } else { 0o644 };
                fs::set_permissions(&abs, fs::Permissions::from_mode(mode)).expect("chmod");
            }
        }
        self.model
            .insert(path.to_string(), (kind, content.to_vec()));
    }

    pub fn remove(&mut self, path: &str) {
        fs::remove_file(self.abs(path)).expect("remove");
        self.model.remove(path).expect("modelled");
    }

    pub fn chmod(&mut self, path: &str, exec: bool) {
        let mode = if exec { 0o755 } else { 0o644 };
        fs::set_permissions(self.abs(path), fs::Permissions::from_mode(mode)).expect("chmod");
        let entry = self.model.get_mut(path).expect("modelled");
        entry.0 = if exec { Kind::Exec } else { Kind::File };
    }

    pub fn rename(&mut self, from: &str, to: &str) {
        let target = self.abs(to);
        fs::create_dir_all(target.parent().expect("has a parent")).expect("mkdir");
        fs::rename(self.abs(from), target).expect("rename");
        let entry = self.model.remove(from).expect("modelled");
        self.model.insert(to.to_string(), entry);
    }

    /// Stage the working directory and return the tree id git computes for it.
    pub fn snapshot(&self) -> GitOid {
        git(self.path(), &["add", "-A"]);
        GitOid::from_hex(&git(self.path(), &["write-tree"])).expect("tree id")
    }

    /// The tree the model describes, built in-process without git.
    pub fn model_tree(&self) -> Tree {
        Tree::from_entries(self.model.iter().map(|(path, (kind, content))| {
            let entry = match kind {
                Kind::File => Entry::file(content),
                Kind::Exec => Entry::executable(content),
                Kind::Link => Entry::symlink(content),
            };
            (RepoPath::new(path.clone()).expect("valid path"), entry)
        }))
        .expect("valid tree")
    }

    pub fn model_blobs(&self) -> Blobs {
        self.model
            .values()
            .map(|(_, content)| (GitOid::for_blob(content), content.clone()))
            .collect()
    }

    /// Check out `tree` into this (empty) repository's working directory and compare
    /// every file on disk with `model`: kind, executable bit, content or link target.
    pub fn check_out_and_compare(
        &self,
        tree: GitOid,
        model: &BTreeMap<String, (Kind, Vec<u8>)>,
    ) -> Result<(), String> {
        git(self.path(), &["read-tree", &tree.to_hex()]);
        git(self.path(), &["checkout-index", "-a", "-f"]);
        let mut on_disk = BTreeMap::new();
        walk(self.path(), self.path(), &mut on_disk);
        if &on_disk != model {
            let a: Vec<_> = on_disk.keys().collect();
            let b: Vec<_> = model.keys().collect();
            return Err(format!(
                "checkout differs from the model: disk {a:?}, model {b:?}"
            ));
        }
        Ok(())
    }
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (Kind, Vec<u8>)>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .expect("under root")
            .to_str()
            .expect("utf-8")
            .to_string();
        if rel == ".git" || rel == ".no-xdg" {
            continue;
        }
        let meta = fs::symlink_metadata(&path).expect("lstat");
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).expect("readlink");
            out.insert(rel, (Kind::Link, target.as_os_str().as_bytes().to_vec()));
        } else if meta.is_dir() {
            walk(root, &path, out);
        } else {
            let kind = if meta.permissions().mode() & 0o111 != 0 {
                Kind::Exec
            } else {
                Kind::File
            };
            out.insert(rel, (kind, fs::read(&path).expect("read")));
        }
    }
}
