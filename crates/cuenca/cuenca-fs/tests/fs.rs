//! Snapshot and checkout against real directories, cross-checked with git.

use cuenca_fs::{
    BlobSource, CheckoutSource, Content, DEFAULT_FORKIGNORE, Fetched, ForkIgnore, FsError,
    ObjectDir, StatCache, checkout, snapshot, snapshot_with,
};
use cuenca_tree::{Entry, GitOid, Tree};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

fn write(root: &Path, path: &str, content: &[u8]) {
    let abs = root.join(path);
    fs::create_dir_all(abs.parent().unwrap()).unwrap();
    fs::write(&abs, content).unwrap();
}

fn set_mode(root: &Path, path: &str, mode: u32) {
    fs::set_permissions(root.join(path), fs::Permissions::from_mode(mode)).unwrap();
}

/// Date every regular file under `root` a minute back, so the stat cache trusts it.
fn age(root: &Path) {
    let old = SystemTime::now() - Duration::from_secs(60);
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for item in fs::read_dir(dir).unwrap() {
            let item = item.unwrap();
            let kind = item.file_type().unwrap();
            if kind.is_dir() {
                dirs.push(item.path());
            } else if kind.is_file() {
                File::open(item.path()).unwrap().set_modified(old).unwrap();
            }
        }
    }
}

/// Every entry kind the format holds, with non-ASCII names at several depths.
fn sample(root: &Path) {
    write(root, "README.md", b"# sample\n");
    write(root, "src/main.rs", b"fn main() {}\n");
    write(root, "bin/run.sh", b"#!/bin/sh\necho hi\n");
    set_mode(root, "bin/run.sh", 0o755);
    write(root, "café/menú.txt", "señor\n".as_bytes());
    write(root, "日本語/ファイル.md", "こんにちは\n".as_bytes());
    write(root, "empty.txt", b"");
    write(root, ".forkignore", DEFAULT_FORKIGNORE.as_bytes());
    symlink("src/main.rs", root.join("link-to-main")).unwrap();
    symlink("/etc/hosts", root.join("absolute-link")).unwrap();
    symlink("../README.md", root.join("src/up")).unwrap();
    symlink("does/not/exist", root.join("dangling")).unwrap();
}

/// The tree id git computes for the directory, as an independent oracle.
fn git_tree_id(root: &Path) -> String {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args([
                "-c",
                "core.precomposeunicode=false",
                "-c",
                "core.fileMode=true",
            ])
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    let id = git(&["write-tree"]);
    fs::remove_dir_all(root.join(".git")).unwrap();
    id
}

/// A source that holds blobs in memory.
struct Memory(HashMap<GitOid, Vec<u8>>);

impl BlobSource for Memory {
    fn fetch(&self, oid: GitOid) -> io::Result<Option<Fetched>> {
        Ok(self.0.get(&oid).cloned().map(Fetched::Bytes))
    }
}

fn memory_of(snap: &cuenca_fs::Snapshot) -> Memory {
    Memory(
        snap.new_blobs
            .iter()
            .map(|b| {
                let bytes = match &b.content {
                    Content::File(p) => fs::read(p).unwrap(),
                    Content::Inline(bytes) => bytes.clone(),
                };
                (b.oid, bytes)
            })
            .collect(),
    )
}

#[test]
fn a_snapshot_is_the_tree_git_computes() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let snap = snapshot(dir.path(), &Tree::new(), &mut StatCache::new()).unwrap();
    assert_eq!(snap.tree.git_tree_id().to_hex(), git_tree_id(dir.path()));
    assert!(matches!(
        snap.tree.get("bin/run.sh"),
        Some(Entry::Executable(_))
    ));
    assert!(matches!(snap.tree.get("dangling"), Some(Entry::Symlink(_))));
    assert_eq!(
        snap.tree.get("link-to-main"),
        Some(&Entry::symlink(b"src/main.rs"))
    );
}

#[test]
fn snapshot_checkout_snapshot_is_the_same_tree_from_every_source() {
    let src = tempfile::tempdir().unwrap();
    sample(src.path());
    age(src.path());
    let mut cache = StatCache::new();
    let snap = snapshot(src.path(), &Tree::new(), &mut cache).unwrap();
    let id = snap.tree.git_tree_id();

    let objects = tempfile::tempdir().unwrap();
    for (oid, bytes) in memory_of(&snap).0 {
        let hex = oid.to_hex();
        write(
            objects.path(),
            &format!("{}/{}", &hex[..2], &hex[2..]),
            &bytes,
        );
    }
    let sources: Vec<(&str, Box<dyn BlobSource>)> = vec![
        (
            "checkout",
            Box::new(CheckoutSource::new(src.path(), &snap.tree, &cache)),
        ),
        ("memory", Box::new(memory_of(&snap))),
        ("objects", Box::new(ObjectDir::new(objects.path()))),
    ];
    for (name, source) in sources {
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("fork");
        let done = checkout(&snap.tree, &dest, &*source).unwrap();
        let mut fork_cache = done.cache;
        let again = snapshot(&dest, &snap.tree, &mut fork_cache).unwrap();
        assert_eq!(again.tree.git_tree_id(), id, "{name}");
        assert!(again.is_clean(), "{name}");
        // What the checkout verified it vouches for; the object store's files it does not.
        let unverified = if name == "objects" { 7 } else { 0 };
        assert_eq!(again.hashed, unverified, "{name}");
        assert_eq!(git_tree_id(&dest), id.to_hex(), "{name}");
        let mode = fs::metadata(dest.join("bin/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "{name}");
        let mode = fs::metadata(dest.join("README.md"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644, "{name}");
    }
}

#[test]
fn forkignored_paths_never_reach_a_changeset() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "keep.txt", b"keep\n");
    for path in [
        ".DS_Store",
        "docs/.DS_Store",
        "._keep.txt",
        "docs/._x",
        ".Spotlight-V100/store.db",
        ".Trashes/501/x",
        ".fseventsd/0001",
        "node_modules/left-pad/index.js",
        "web/node_modules/x.js",
        "target/debug/app",
        "crates/a/target/x",
        ".venv/bin/python",
        ".env",
        "app/.env",
    ] {
        write(root, path, b"host-local\n");
    }
    // No .forkignore: the defaults apply.
    let snap = snapshot(root, &Tree::new(), &mut StatCache::new()).unwrap();
    let paths: Vec<&str> = snap.tree.entries().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["keep.txt"]);
    let ops: Vec<&str> = snap
        .changeset
        .ops
        .iter()
        .map(|o| o.path().as_str())
        .collect();
    assert_eq!(ops, ["keep.txt"]);

    // A .forkignore that exists is the whole rule set.
    write(root, ".forkignore", b"*.log\n!keep.log\n");
    write(root, "a.log", b"x\n");
    write(root, "keep.log", b"y\n");
    let snap = snapshot(root, &Tree::new(), &mut StatCache::new()).unwrap();
    assert!(snap.tree.get("a.log").is_none());
    assert!(snap.tree.get("keep.log").is_some());
    assert!(
        snap.tree.get(".env").is_some(),
        "the location dropped the default"
    );
    assert!(
        snap.tree.get(".forkignore").is_some(),
        "the rules are the location's content"
    );
}

#[test]
fn the_stat_cache_hashes_only_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..20 {
        write(root, &format!("f{i}.txt"), format!("file {i}\n").as_bytes());
    }
    age(root);
    let mut cache = StatCache::new();
    let first = snapshot(root, &Tree::new(), &mut cache).unwrap();
    assert_eq!(first.hashed, 20);
    let again = snapshot(root, &first.tree, &mut cache).unwrap();
    assert_eq!(again.hashed, 0);
    assert!(again.is_clean());

    // Same size, modification time put back: the change time still moves.
    let old = fs::metadata(root.join("f3.txt"))
        .unwrap()
        .modified()
        .unwrap();
    write(root, "f3.txt", b"FILE 3\n");
    File::open(root.join("f3.txt"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let edited = snapshot(root, &first.tree, &mut cache).unwrap();
    assert_eq!(edited.hashed, 1);
    assert_eq!(edited.tree.get("f3.txt"), Some(&Entry::file(b"FILE 3\n")));

    // A mode change alone is a change of kind.
    set_mode(root, "f4.txt", 0o755);
    let chmod = snapshot(root, &edited.tree, &mut cache).unwrap();
    assert_eq!(
        chmod.tree.get("f4.txt"),
        Some(&Entry::executable(b"file 4\n"))
    );
    assert_eq!(chmod.changeset.ops.len(), 1);
    assert!(chmod.new_blobs.is_empty(), "the content was already known");
}

#[test]
fn a_file_written_just_now_is_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "fresh.txt", b"aaaa\n");
    let mut cache = StatCache::new();
    snapshot(dir.path(), &Tree::new(), &mut cache).unwrap();
    assert!(
        cache.is_empty(),
        "within the racy margin, the stat proves nothing"
    );
    let again = snapshot(dir.path(), &Tree::new(), &mut cache).unwrap();
    assert_eq!(again.hashed, 1);
}

#[test]
fn new_blobs_are_exactly_those_the_base_lacks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a", b"same\n");
    write(root, "b", b"same\n");
    let base = snapshot(root, &Tree::new(), &mut StatCache::new()).unwrap();
    assert_eq!(base.new_blobs.len(), 1, "one blob named twice is sent once");
    write(root, "c", b"same\n");
    write(root, "d", b"new\n");
    symlink("a", root.join("e")).unwrap();
    let next = snapshot(root, &base.tree, &mut StatCache::new()).unwrap();
    let mut oids: Vec<GitOid> = next.new_blobs.iter().map(|b| b.oid).collect();
    oids.sort();
    let mut want = vec![Entry::file(b"new\n").oid(), Entry::symlink(b"a").oid()];
    want.sort();
    assert_eq!(oids, want);
    let inline = next
        .new_blobs
        .iter()
        .find(|b| b.oid == Entry::symlink(b"a").oid())
        .unwrap();
    assert_eq!(inline.content, Content::Inline(b"a".to_vec()));
    assert_eq!(inline.size, 1);
}

#[test]
fn git_state_is_left_out_and_special_files_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "file", b"x\n");
    write(root, ".git/HEAD", b"ref: refs/heads/main\n");
    write(root, "sub/.git", b"gitdir: ../elsewhere\n");
    let socket = root.join("sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let fifo = Command::new("mkfifo")
        .arg(root.join("pipe"))
        .status()
        .unwrap();
    assert!(fifo.success());
    let snap = snapshot(root, &Tree::new(), &mut StatCache::new()).unwrap();
    let paths: Vec<&str> = snap.tree.entries().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["file"]);
    let mut skipped: Vec<String> = snap.skipped.iter().map(|s| s.path.clone()).collect();
    skipped.sort();
    assert_eq!(skipped, ["pipe", "sock"]);
}

#[test]
fn what_a_tree_cannot_hold_fails_loudly() {
    // A name git reserves under an alias is listed, not silently dropped.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "ok", b"x\n");
    write(dir.path(), "git~1/x", b"x\n");
    let snap = snapshot(dir.path(), &Tree::new(), &mut StatCache::new()).unwrap();
    assert_eq!(snap.skipped.len(), 1);

    // .gitmodules cannot be in a tree.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".gitmodules", b"[submodule]\n");
    assert!(matches!(
        snapshot(dir.path(), &Tree::new(), &mut StatCache::new()),
        Err(FsError::Tree(_))
    ));

    // A backslash is a separator on SMB.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a\\b", b"x\n");
    assert!(matches!(
        snapshot(dir.path(), &Tree::new(), &mut StatCache::new()),
        Err(FsError::Name { .. })
    ));
}

#[test]
fn an_unreadable_file_fails_the_snapshot_with_its_path() {
    if rustix::process::geteuid().is_root() {
        return; // root reads everything
    }
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "secret", b"x\n");
    set_mode(dir.path(), "secret", 0o000);
    let mut cache = StatCache::new();
    match snapshot(dir.path(), &Tree::new(), &mut cache) {
        Err(FsError::Io { path, .. }) => assert!(path.ends_with("secret")),
        other => panic!("expected an I/O error naming the file, got {other:?}"),
    }
    set_mode(dir.path(), "secret", 0o644);
}

#[test]
fn a_failed_snapshot_leaves_the_cache_alone() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a", b"x\n");
    age(dir.path());
    let mut cache = StatCache::new();
    snapshot(dir.path(), &Tree::new(), &mut cache).unwrap();
    let before = cache.clone();
    write(dir.path(), ".gitmodules", b"x\n");
    assert!(snapshot(dir.path(), &Tree::new(), &mut cache).is_err());
    assert_eq!(cache, before);
}

#[test]
fn checkout_refuses_a_non_empty_destination() {
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("dest");
    fs::create_dir(&dest).unwrap();
    set_mode(out.path(), "dest", 0o700);
    write(&dest, "already", b"x\n");
    let tree = Tree::new();
    match checkout(&tree, &dest, &Memory(HashMap::new())) {
        Err(FsError::Destination {
            why: "it is not empty",
            ..
        }) => {}
        other => panic!("expected a refusal for a non-empty destination, got {other:?}"),
    }
}

#[test]
fn checkout_refuses_wrong_or_missing_content() {
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "a.txt", b"right\n");
    let snap = snapshot(src.path(), &Tree::new(), &mut StatCache::new()).unwrap();
    let oid = snap.tree.get("a.txt").unwrap().oid();

    let out = tempfile::tempdir().unwrap();
    let lying = Memory(HashMap::from([(oid, b"wrong\n".to_vec())]));
    assert!(matches!(
        checkout(&snap.tree, &out.path().join("x"), &lying),
        Err(FsError::BlobMismatch { .. })
    ));
    assert!(matches!(
        checkout(&snap.tree, &out.path().join("y"), &Memory(HashMap::new())),
        Err(FsError::MissingBlob(missing)) if missing == oid
    ));

    // A symlink's target is checked too, whatever kind of source hands it over.
    let dir = tempfile::tempdir().unwrap();
    symlink("t", dir.path().join("l")).unwrap();
    let snap = snapshot(dir.path(), &Tree::new(), &mut StatCache::new()).unwrap();
    let objects = tempfile::tempdir().unwrap();
    let hex = snap.tree.get("l").unwrap().oid().to_hex();
    write(
        objects.path(),
        &format!("{}/{}", &hex[..2], &hex[2..]),
        b"u",
    );
    assert!(matches!(
        checkout(
            &snap.tree,
            &out.path().join("z"),
            &ObjectDir::new(objects.path())
        ),
        Err(FsError::BlobMismatch { .. })
    ));
}

#[test]
fn a_checkout_source_never_offers_a_file_edited_since_its_snapshot() {
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "a.txt", b"v1\n");
    age(src.path());
    let mut cache = StatCache::new();
    let snap = snapshot(src.path(), &Tree::new(), &mut cache).unwrap();
    write(src.path(), "a.txt", b"v2\n");
    let source = CheckoutSource::new(src.path(), &snap.tree, &cache);
    let oid = snap.tree.get("a.txt").unwrap().oid();
    assert!(source.fetch(oid).unwrap().is_none());
    let out = tempfile::tempdir().unwrap();
    assert!(matches!(
        checkout(&snap.tree, &out.path().join("f"), &source),
        Err(FsError::MissingBlob(_))
    ));
}

#[test]
fn drift_from_a_checkout_is_seen() {
    let src = tempfile::tempdir().unwrap();
    sample(src.path());
    let snap = snapshot(src.path(), &Tree::new(), &mut StatCache::new()).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("fork");
    let mut cache = checkout(&snap.tree, &dest, &memory_of(&snap))
        .unwrap()
        .cache;
    assert!(snapshot(&dest, &snap.tree, &mut cache).unwrap().is_clean());
    fs::remove_file(dest.join("README.md")).unwrap();
    fs::rename(dest.join("src/main.rs"), dest.join("src/lib.rs")).unwrap();
    let drift = snapshot(&dest, &snap.tree, &mut cache).unwrap();
    let ops: Vec<&str> = drift
        .changeset
        .ops
        .iter()
        .map(|o| o.path().as_str())
        .collect();
    assert_eq!(ops, ["README.md", "src/lib.rs", "src/main.rs"]);
    assert!(drift.new_blobs.is_empty(), "a rename sends no content");
}

/// Spec row 2·B: a snapshot after an agent writes 1,000 files is published in ≤ 5 s.
/// This measures the scan, which is the part this crate owns.
#[test]
fn a_snapshot_after_writing_1000_files_takes_under_five_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..2000 {
        write(
            root,
            &format!("old/{}/{i}.txt", i % 40),
            format!("old {i}\n").as_bytes(),
        );
    }
    age(root);
    let mut cache = StatCache::new();
    let base = snapshot(root, &Tree::new(), &mut cache).unwrap();
    for i in 0..1000 {
        write(
            root,
            &format!("new/{}/{i}.rs", i % 25),
            format!("// {i}\n{}\n", "x".repeat(i)).as_bytes(),
        );
    }
    let start = Instant::now();
    let snap = snapshot(root, &base.tree, &mut cache).unwrap();
    let took = start.elapsed();
    assert_eq!(snap.changeset.ops.len(), 1000);
    assert_eq!(snap.hashed, 1000);
    eprintln!("snapshot of 1,000 new files over 2,000 cached: {took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn explicit_rules_override_the_location_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".forkignore", b"");
    write(dir.path(), "x.tmp", b"x\n");
    let rules = ForkIgnore::parse("*.tmp\n").unwrap();
    let snap = snapshot_with(dir.path(), &Tree::new(), &mut StatCache::new(), &rules).unwrap();
    assert!(snap.tree.get("x.tmp").is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn a_name_that_is_not_utf8_fails_loudly() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(OsStr::from_bytes(b"bad\xff")), b"x").unwrap();
    assert!(matches!(
        snapshot(dir.path(), &Tree::new(), &mut StatCache::new()),
        Err(FsError::NotUtf8(_))
    ));
}

#[test]
fn a_gitattributes_git_cannot_parse_fails_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "docs/.gitattributes",
        format!("*.md {}\n", "x".repeat(3000)).as_bytes(),
    );
    assert!(matches!(
        snapshot(dir.path(), &Tree::new(), &mut StatCache::new()),
        Err(FsError::Tree(_))
    ));
}

#[test]
fn a_file_over_the_blob_limit_fails_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    // Sparse, so it costs no disk.
    let big = File::create(dir.path().join("big.bin")).unwrap();
    big.set_len(cuenca_fs::MAX_FILE_BYTES + 1).unwrap();
    match snapshot(dir.path(), &Tree::new(), &mut StatCache::new()) {
        Err(FsError::TooLarge { path, .. }) => assert!(path.ends_with("big.bin")),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

/// A source whose file no longer has the stamp it offered it under.
struct Moved {
    path: std::path::PathBuf,
    stamp: cuenca_fs::Stamp,
}

impl BlobSource for Moved {
    fn fetch(&self, _: GitOid) -> io::Result<Option<Fetched>> {
        Ok(Some(Fetched::File {
            file: File::open(&self.path)?,
            stamp: Some(self.stamp),
        }))
    }
}

#[test]
fn a_clone_whose_source_changed_is_refused() {
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "a.txt", b"content\n");
    write(src.path(), "other", b"other\n");
    let snap = snapshot_with(
        src.path(),
        &Tree::new(),
        &mut StatCache::new(),
        &ForkIgnore::parse("other").unwrap(),
    )
    .unwrap();
    let source = Moved {
        path: src.path().join("a.txt"),
        stamp: cuenca_fs::Stamp::of(&fs::symlink_metadata(src.path().join("other")).unwrap()),
    };
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("fork");
    assert!(matches!(
        checkout(&snap.tree, &dest, &source),
        Err(FsError::SourceChanged(_))
    ));
    assert!(!dest.join("a.txt").exists(), "the clone is removed");
}

#[test]
fn ignored_paths_of_the_base_are_carried_not_deleted() {
    // A tree that holds paths the defaults ignore (imported, or from rules that kept them).
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "src/lib.rs", b"pub fn f() {}\n");
    write(src.path(), "src/target/mod.rs", b"// tracked\n");
    write(
        src.path(),
        "node_modules/vendored/index.js",
        b"module.exports = 1;\n",
    );
    let keep_all = ForkIgnore::parse("").unwrap();
    let base = snapshot_with(src.path(), &Tree::new(), &mut StatCache::new(), &keep_all).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("fork");
    let mut cache = checkout(&base.tree, &dest, &memory_of(&base))
        .unwrap()
        .cache;
    // No .forkignore: the defaults ignore both, and the snapshot carries them.
    let snap = snapshot(&dest, &base.tree, &mut cache).unwrap();
    assert!(snap.is_clean(), "{:?}", snap.changeset.ops);
    assert_eq!(snap.tree.git_tree_id(), base.tree.git_tree_id());
    // Editing an ignored file changes nothing the snapshot claims.
    write(&dest, "src/target/mod.rs", b"// edited, host-local now\n");
    assert!(snapshot(&dest, &base.tree, &mut cache).unwrap().is_clean());
    // A deletion outside the ignored paths is still seen.
    fs::remove_file(dest.join("src/lib.rs")).unwrap();
    let ops: Vec<String> = snapshot(&dest, &base.tree, &mut cache)
        .unwrap()
        .changeset
        .ops
        .iter()
        .map(|o| o.path().to_string())
        .collect();
    assert_eq!(ops, ["src/lib.rs"]);
}

#[test]
fn a_checkout_source_serves_files_too_new_to_be_cached() {
    let src = tempfile::tempdir().unwrap();
    sample(src.path());
    let mut cache = StatCache::new();
    let snap = snapshot(src.path(), &Tree::new(), &mut cache).unwrap();
    assert!(cache.is_empty(), "just written, nothing is settled");
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("fork");
    let done = checkout(
        &snap.tree,
        &dest,
        &CheckoutSource::new(src.path(), &snap.tree, &cache),
    )
    .unwrap();
    let mut fork_cache = done.cache;
    let again = snapshot(&dest, &snap.tree, &mut fork_cache).unwrap();
    assert!(again.is_clean());
    assert_eq!(
        again.hashed, 0,
        "bytes read and hashed by the source are vouched for"
    );
}

#[test]
fn a_checkout_source_tries_every_path_with_the_content() {
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "a", b"same\n");
    write(src.path(), "b", b"same\n");
    age(src.path());
    let mut cache = StatCache::new();
    let snap = snapshot(src.path(), &Tree::new(), &mut cache).unwrap();
    write(src.path(), "a", b"edited\n");
    let out = tempfile::tempdir().unwrap();
    let done = checkout(
        &snap.tree,
        &out.path().join("fork"),
        &CheckoutSource::new(src.path(), &snap.tree, &cache),
    )
    .unwrap();
    assert_eq!(fs::read(out.path().join("fork/a")).unwrap(), b"same\n");
    // Both from the intact `b`, offered as a file the source vouches for (cloned or, on a
    // disk that cannot clone, copied), so the new cache vouches for both.
    assert_eq!(done.cloned + done.copied, 2);
    assert_eq!(done.cache.len(), 2);
}

#[test]
fn a_corrupt_object_is_not_vouched_for() {
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "a.txt", b"right content\n");
    let snap = snapshot(src.path(), &Tree::new(), &mut StatCache::new()).unwrap();
    let objects = tempfile::tempdir().unwrap();
    let hex = snap.tree.get("a.txt").unwrap().oid().to_hex();
    write(
        objects.path(),
        &format!("{}/{}", &hex[..2], &hex[2..]),
        b"TRUNC",
    );
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("fork");
    let mut cache = checkout(&snap.tree, &dest, &ObjectDir::new(objects.path()))
        .unwrap()
        .cache;
    assert!(cache.is_empty());
    let first = snapshot(&dest, &snap.tree, &mut cache).unwrap();
    assert!(
        !first.is_clean(),
        "the first snapshot reads the file and sees the damage"
    );
}

#[test]
fn forkignore_rules_can_leave_out_a_name_no_tree_holds() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "weird\\name.txt", b"x\n");
    write(dir.path(), "ok.txt", b"y\n");
    write(dir.path(), ".forkignore", b"weird*\n");
    let snap = snapshot(dir.path(), &Tree::new(), &mut StatCache::new()).unwrap();
    let paths: Vec<&str> = snap.tree.entries().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, [".forkignore", "ok.txt"]);
}

#[test]
fn checkout_refuses_a_destination_it_does_not_own_alone() {
    let out = tempfile::tempdir().unwrap();
    let tree = Tree::new();
    let empty = Memory(HashMap::new());
    fs::create_dir(out.path().join("real")).unwrap();
    symlink("real", out.path().join("link")).unwrap();
    assert!(matches!(
        checkout(&tree, &out.path().join("link"), &empty),
        Err(FsError::Destination {
            why: "it is a symlink",
            ..
        })
    ));
    fs::create_dir(out.path().join("shared")).unwrap();
    set_mode(out.path(), "shared", 0o775);
    assert!(matches!(
        checkout(&tree, &out.path().join("shared"), &empty),
        Err(FsError::Destination {
            why: "group or others can write it",
            ..
        })
    ));
    write(out.path(), "file", b"x");
    assert!(matches!(
        checkout(&tree, &out.path().join("file"), &empty),
        Err(FsError::Destination {
            why: "it is not a directory",
            ..
        })
    ));
}
