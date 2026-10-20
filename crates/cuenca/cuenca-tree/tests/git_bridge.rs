//! The git bridge against trees a working directory cannot produce, against git and
//! APFS as oracles for the path rules, and against the repository this crate is in.

#![cfg(unix)]

mod common;

use common::{Work, git};
use cuenca_tree::git::{Blobs, GitError, Repo};
use cuenca_tree::{Blob, Entry, GitOid, RepoPath, Tree, TreeError};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Store `content` as a blob and return its id.
fn blob(w: &Work, content: &str) -> String {
    let file = w.path().join(".blob");
    std::fs::write(&file, content).unwrap();
    git(w.path(), &["hash-object", "-w", "--no-filters", ".blob"])
}

/// Stage entries straight into the index, bypassing the filesystem (which on macOS
/// could not hold a case collision), and write the tree. git on macOS precomposes argv
/// to NFC (`core.precomposeunicode`), which would turn a decomposed name into its
/// composed twin before it reached the index, so that is switched off.
fn tree_from_index(w: &Work, entries: &[(&str, &str, &str)]) -> GitOid {
    for (mode, path, content) in entries {
        let oid = if *mode == "160000" {
            content.to_string()
        } else {
            blob(w, content)
        };
        let info = format!("{mode},{oid},{path}");
        git(
            w.path(),
            &[
                "-c",
                "core.precomposeunicode=false",
                "update-index",
                "--add",
                "--cacheinfo",
                &info,
            ],
        );
    }
    GitOid::from_hex(&git(w.path(), &["write-tree"])).unwrap()
}

/// Whether git, with both of its checkout protections on, accepts `path` into the index.
fn git_accepts(w: &Work, mode: &str, path: &str) -> bool {
    let oid = GitOid::for_blob(b"x").to_hex();
    let status = Command::new("git")
        .arg("-C")
        .arg(w.path())
        .args([
            "-c",
            "core.protectHFS=true",
            "-c",
            "core.protectNTFS=true",
            "-c",
            "core.precomposeunicode=false",
        ])
        .args([
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("{mode},{oid},{path}"),
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stderr(Stdio::null())
        .status()
        .unwrap();
    status.success()
}

fn mktree(w: &Work, lines: &str, missing: bool) -> String {
    let mut args = vec![
        "-C".to_string(),
        w.path().to_str().unwrap().to_string(),
        "mktree".to_string(),
    ];
    if missing {
        args.push("--missing".into());
    }
    let mut child = Command::new("git")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_string()
}

/// Path names on which cuenca claims parity with git's `verify_path`: refused by one
/// exactly when refused by the other.
const PATH_CORPUS: &[&str] = &[
    ".git",
    ".GIT",
    ".git.",
    ".git ",
    ".git . .",
    ".git:x",
    ".git :x",
    ".git::$INDEX_ALLOCATION",
    "git~1",
    "GIT~1",
    "git~1.",
    "GIT~1 ",
    "git~1:stream",
    ".g\u{200C}it",
    ".gi\u{FEFF}t",
    "a/.git/b",
    "a/git~1/b",
    ".gitignore",
    ".github",
    ".gitx",
    "git~2",
    "gitx",
    "x.git",
    "git",
    "ok",
    "a/b/c",
    "sp ace",
    "ñandú",
];

#[test]
fn git_dir_rules_match_gits_own() {
    let w = Work::new();
    let mut disagreements = Vec::new();
    for path in PATH_CORPUS {
        let ours = RepoPath::new(*path).is_ok();
        let theirs = git_accepts(&w, "100644", path);
        if ours != theirs {
            disagreements.push(format!("{path:?}: cuenca {ours}, git {theirs}"));
        }
    }
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n"));
}

#[test]
fn symlinked_gitmodules_rules_match_gits_own() {
    let w = Work::new();
    let mut disagreements = Vec::new();
    for name in [
        ".gitmodules",
        ".GITMODULES",
        ".gitmodules.",
        "gitmod~1",
        "GITMOD~4",
        "gi7eba~9",
        ".gitmodulesx",
        "gitmod~5",
        "modules",
    ] {
        let tree = Tree::from_entries([(RepoPath::new(name).unwrap(), Entry::symlink(b"target"))]);
        let ours = tree.is_ok();
        let theirs = git_accepts(&w, "120000", name);
        if ours != theirs {
            disagreements.push(format!("{name:?}: cuenca {ours}, git {theirs}"));
        }
    }
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n"));
}

/// On a case-insensitive APFS volume, the filesystem itself is the oracle: if creating
/// one name makes the other resolve, cuenca must refuse to hold both.
#[cfg(target_os = "macos")]
#[test]
fn fold_collisions_match_apfs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("probe"), "").unwrap();
    if !dir.path().join("PROBE").exists() {
        eprintln!("skipped: the temp volume is case-sensitive");
        return;
    }
    let pairs = [
        ("README", "readme"),
        ("caf\u{e9}", "cafe\u{301}"),
        ("straße", "strasse"),
        ("aς", "aσ"),
        ("ΑΣ", "ασ"),
        ("ﬁ", "fi"),
        ("ϐ", "β"),
        ("ſ", "s"),
        ("µ", "μ"),
        ("ϑ", "θ"),
        ("ϰ", "κ"),
        ("ϱ", "ρ"),
        ("ϵ", "ε"),
        ("a", "b"),
        ("e", "\u{e9}"),
        ("i", "ı"),
    ];
    let mut disagreements = Vec::new();
    for (i, (a, b)) in pairs.iter().enumerate() {
        let sub = dir.path().join(format!("p{i}"));
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join(a), "").unwrap();
        let apfs_collides = sub.join(b).exists();
        let tree = Tree::from_entries([
            (RepoPath::new(*a).unwrap(), Entry::file(b"1")),
            (RepoPath::new(*b).unwrap(), Entry::file(b"2")),
        ]);
        let cuenca_collides = matches!(tree, Err(TreeError::Collision { .. }));
        if apfs_collides != cuenca_collides {
            disagreements.push(format!(
                "{a:?}/{b:?}: APFS {apfs_collides}, cuenca {cuenca_collides}"
            ));
        }
    }
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n"));
}

#[test]
fn a_case_collision_in_git_is_refused() {
    let w = Work::new();
    let t = tree_from_index(
        &w,
        &[("100644", "README.md", "a"), ("100644", "readme.md", "b")],
    );
    let err = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::Collision { .. })),
        "{err}"
    );
}

#[test]
fn a_normalisation_collision_in_git_is_refused() {
    let w = Work::new();
    let t = tree_from_index(
        &w,
        &[("100644", "caf\u{e9}", "a"), ("100644", "cafe\u{301}", "b")],
    );
    let err = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::Collision { .. })),
        "{err}"
    );
}

#[test]
fn a_submodule_is_refused() {
    let w = Work::new();
    let commit = "1111111111111111111111111111111111111111";
    let t = tree_from_index(
        &w,
        &[("160000", "vendor/sub", commit), ("100644", "a.txt", "a")],
    );
    let err = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::UnsupportedMode(_))),
        "{err}"
    );
}

#[test]
fn an_empty_directory_is_refused() {
    let w = Work::new();
    let empty = mktree(&w, "", false);
    assert_eq!(empty, GitOid::empty_tree().to_hex());
    let a = blob(&w, "a");
    let root = mktree(
        &w,
        &format!("040000 tree {empty}\tempty\n100644 blob {a}\ta.txt\n"),
        false,
    );
    let err = Repo::open(w.path()).read_tree(&root).unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::EmptyDirectory(ref d)) if d == "empty"),
        "{err}"
    );
}

#[test]
fn replace_refs_cannot_swap_a_blob() {
    let w = Work::new();
    let t = tree_from_index(&w, &[("100644", "a.txt", "original")]);
    let evil = blob(&w, "replaced");
    let original = GitOid::for_blob(b"original").to_hex();
    git(w.path(), &["replace", &original, &evil]);
    assert_eq!(
        git(w.path(), &["cat-file", "-p", &original]),
        "replaced",
        "git itself follows the replace ref"
    );
    let tree = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap();
    assert_eq!(tree.get("a.txt"), Some(&Entry::file(b"original")));
    let blobs = Repo::open(w.path())
        .read_blobs([GitOid::for_blob(b"original")])
        .unwrap();
    assert_eq!(blobs.values().next().unwrap(), b"original");
}

#[test]
fn writing_refuses_content_that_does_not_match_its_entry() {
    let tree = Tree::from_entries([(RepoPath::new("a").unwrap(), Entry::file(b"real"))]).unwrap();
    let mut blobs = Blobs::new();
    blobs.insert(GitOid::for_blob(b"real"), b"forged".to_vec());
    let w = Work::new();
    let err = Repo::open(w.path()).write_tree(&tree, &blobs).unwrap_err();
    assert!(matches!(err, GitError::Corrupt(_)), "{err}");
    let err = Repo::open(w.path())
        .write_tree(&tree, &Default::default())
        .unwrap_err();
    assert!(matches!(err, GitError::NoContent(_)), "{err}");
}

/// Stratum B's attack: the repository already holds an old, vulnerable blob; the entry
/// names it by git id but claims the SHA-256 and size of the current content.
#[test]
fn writing_verifies_blobs_the_repository_already_has() {
    let w = Work::new();
    blob(&w, "vulnerable old content");
    let current = Blob::of(b"fixed content");
    let forged = Blob {
        oid: GitOid::for_blob(b"vulnerable old content"),
        ..current
    };
    let tree =
        Tree::from_entries([(RepoPath::new("auth.rs").unwrap(), Entry::File(forged))]).unwrap();
    let err = Repo::open(w.path())
        .write_tree(&tree, &Blobs::new())
        .unwrap_err();
    assert!(
        matches!(err, GitError::Corrupt(oid) if oid == forged.oid),
        "{err}"
    );
}

#[test]
fn the_empty_tree_round_trips() {
    let w = Work::new();
    let written = Repo::open(w.path())
        .write_tree(&Tree::new(), &Default::default())
        .unwrap();
    assert_eq!(written, GitOid::empty_tree());
}

#[test]
fn a_blob_missing_from_the_object_store_is_reported() {
    let w = Work::new();
    let absent = GitOid::for_blob(b"never stored");
    let tree = mktree(&w, &format!("100644 blob {absent}\tgone.txt\n"), true);
    let err = Repo::open(w.path()).read_tree(&tree).unwrap_err();
    assert!(
        matches!(err, GitError::Missing(oid) if oid == absent),
        "{err}"
    );
}

/// A real `blob:none` partial clone of a local repository: reading must report the blob
/// missing and must not fetch it, so the clone still lacks it afterwards.
#[test]
fn a_partial_clone_is_not_fetched_from() {
    let src = Work::new();
    std::fs::write(src.path().join("a.txt"), "content only the source has").unwrap();
    git(src.path(), &["add", "a.txt"]);
    git(
        src.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "a",
        ],
    );
    git(src.path(), &["config", "uploadpack.allowFilter", "true"]);
    let dst = tempfile::tempdir().unwrap();
    let url = format!("file://{}", src.path().display());
    git(
        dst.path(),
        &[
            "clone",
            "-q",
            "--no-checkout",
            "--filter=blob:none",
            &url,
            "clone",
        ],
    );
    let clone = dst.path().join("clone");
    let oid = GitOid::for_blob(b"content only the source has");

    let err = Repo::open(&clone).read_tree("HEAD").unwrap_err();
    assert!(matches!(err, GitError::Missing(o) if o == oid), "{err}");
    let still_absent = Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["cat-file", "-e", &oid.to_hex()])
        .env("GIT_NO_LAZY_FETCH", "1")
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!still_absent.success(), "the blob was fetched");
}

/// The repository this crate is checked out in: its HEAD tree, read through the bridge,
/// must hash to the id git holds. It has executables and symlinks across many levels.
#[test]
fn this_repository_head_round_trips() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let top = git(here, &["rev-parse", "--show-toplevel"]);
    let repo = Repo::open(&top);
    let expected = repo.resolve_tree("HEAD").unwrap();
    let tree = repo.read_tree("HEAD").unwrap();
    assert_eq!(tree.git_tree_id(), expected);
    assert!(
        tree.len() > 100,
        "expected a real tree, got {} entries",
        tree.len()
    );
}

#[test]
fn a_hostile_deep_path_is_refused_before_any_work() {
    let deep = vec!["a"; 16_000].join("/");
    let json = format!(
        "{{\"{deep}\":{}}}",
        serde_json::to_string(&Entry::file(b"x")).unwrap()
    );
    let started = std::time::Instant::now();
    assert!(serde_json::from_str::<Tree>(&json).is_err());
    assert!(
        started.elapsed().as_millis() < 500,
        "took {:?}",
        started.elapsed()
    );
}

/// A loose object whose bytes are another blob's (disk corruption, or a hostile fork
/// writing into its own object store): git serves it under the requested id without
/// checking, so cuenca must.
#[test]
fn a_corrupt_loose_object_is_detected() {
    let w = Work::new();
    let real = GitOid::for_blob(b"real\n");
    let t = mktree(&w, &format!("100644 blob {real}\ta.txt\n"), true);
    let evil = blob(&w, "evil\n");
    let objects = w.path().join(".git/objects");
    let (from, to) = (
        objects.join(&evil[..2]).join(&evil[2..]),
        objects.join(&real.to_hex()[..2]).join(&real.to_hex()[2..]),
    );
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::copy(from, to).unwrap();
    let repo = Repo::open(w.path());
    assert!(matches!(repo.read_tree(&t), Err(GitError::Corrupt(o)) if o == real));
    assert!(matches!(repo.read_blobs([real]), Err(GitError::Corrupt(o)) if o == real));
}
