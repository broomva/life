//! The git bridge against trees a working directory cannot produce, and against the
//! repository this crate is checked out in.

#![cfg(unix)]

mod common;

use common::{Work, git};
use cuenca_tree::git::{GitError, Repo};
use cuenca_tree::{Entry, GitOid, RepoPath, Tree, TreeError};
use std::path::Path;

/// Stage `paths` straight into the index as blobs with the given content, bypassing the
/// filesystem (which on macOS could not hold a case collision), and write the tree.
fn tree_from_index(w: &Work, entries: &[(&str, &str, &str)]) -> GitOid {
    for (mode, path, content) in entries {
        let oid = if *mode == "160000" {
            content.to_string()
        } else {
            let file = w.path().join(".blob");
            std::fs::write(&file, content).unwrap();
            git(w.path(), &["hash-object", "-w", "--no-filters", ".blob"])
        };
        // git on macOS precomposes argv to NFC (`core.precomposeunicode`), which would turn
        // a decomposed name into its composed twin before it reached the index.
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
fn a_gitlink_round_trips() {
    let w = Work::new();
    let commit = "1111111111111111111111111111111111111111";
    let t = tree_from_index(
        &w,
        &[("160000", "vendor/sub", commit), ("100644", "a.txt", "a")],
    );
    let tree = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap();
    assert_eq!(
        tree.get("vendor/sub"),
        Some(&Entry::Gitlink(GitOid::from_hex(commit).unwrap()))
    );
    let fresh = Work::new();
    let blobs = Repo::open(w.path()).read_tree_blobs(&tree).unwrap();
    assert_eq!(
        Repo::open(fresh.path()).write_tree(&tree, &blobs).unwrap(),
        t
    );
}

#[test]
fn replace_refs_cannot_swap_a_blob() {
    let w = Work::new();
    let t = tree_from_index(&w, &[("100644", "a.txt", "original")]);
    std::fs::write(w.path().join(".evil"), "replaced").unwrap();
    let evil = git(w.path(), &["hash-object", "-w", ".evil"]);
    let original = GitOid::for_blob(b"original").to_hex();
    git(w.path(), &["replace", &original, &evil]);
    assert_eq!(
        git(w.path(), &["cat-file", "-p", &original]),
        "replaced",
        "git itself follows the replace ref"
    );
    let tree = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap();
    assert_eq!(tree.get("a.txt"), Some(&Entry::file(b"original")));
}

#[test]
fn writing_refuses_content_that_does_not_match_its_entry() {
    let tree = Tree::from_entries([(RepoPath::new("a").unwrap(), Entry::file(b"real"))]).unwrap();
    let mut blobs = cuenca_tree::git::Blobs::new();
    blobs.insert(GitOid::for_blob(b"real"), b"forged".to_vec());
    let w = Work::new();
    let err = Repo::open(w.path()).write_tree(&tree, &blobs).unwrap_err();
    assert!(matches!(err, GitError::Corrupt(_)), "{err}");
    let err = Repo::open(w.path())
        .write_tree(&tree, &Default::default())
        .unwrap_err();
    assert!(matches!(err, GitError::NoContent(_)), "{err}");
}

#[test]
fn the_empty_tree_round_trips() {
    let w = Work::new();
    assert_eq!(
        Repo::open(w.path())
            .write_tree(&Tree::new(), &Default::default())
            .unwrap(),
        GitOid::empty_tree()
    );
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
fn a_blob_missing_from_the_object_store_is_reported_not_fetched() {
    let w = Work::new();
    let absent = GitOid::for_blob(b"never stored");
    let line = format!("100644 blob {absent}\tgone.txt\n");
    let tree = std::process::Command::new("git")
        .arg("-C")
        .arg(w.path())
        .args(["mktree", "--missing"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(line.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    let tree = String::from_utf8(tree.stdout).unwrap();
    let err = Repo::open(w.path()).read_tree(tree.trim()).unwrap_err();
    assert!(
        matches!(err, GitError::Missing(oid) if oid == absent),
        "{err}"
    );
}
