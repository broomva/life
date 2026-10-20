//! The git bridge against trees a working directory cannot produce; git, `git fsck` and
//! APFS as oracles for the path rules; and the repository this crate is checked out in.

#![cfg(unix)]

mod common;

use common::{Work, git};
use cuenca_tree::git::{Blobs, GitError, Repo};
use cuenca_tree::{Entry, GitOid, RepoPath, Tree, TreeError};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Store `content` as a blob and return its id.
fn blob(w: &Work, content: &[u8]) -> String {
    let file = w.path().join(".blob");
    std::fs::write(&file, content).unwrap();
    git(w.path(), &["hash-object", "-w", "--no-filters", ".blob"])
}

/// One index entry: mode, path and content (a commit id, for mode `160000`).
type IndexEntry<'a> = (&'a str, &'a str, &'a [u8]);

/// Stage entries straight into the index, bypassing the filesystem (which on macOS
/// could not hold a case collision), and write the tree. git on macOS precomposes argv
/// to NFC (`core.precomposeunicode`), which would turn a decomposed name into its
/// composed twin before it reached the index, so that is switched off.
fn tree_from_index(w: &Work, entries: &[IndexEntry]) -> GitOid {
    for (mode, path, content) in entries {
        let oid = if *mode == "160000" {
            String::from_utf8(content.to_vec()).unwrap()
        } else {
            blob(w, content)
        };
        let info = format!("{mode},{oid},{path}");
        git(
            w.path(),
            &[
                "-c",
                "core.precomposeunicode=false",
                "-c",
                "core.protectNTFS=false",
                "update-index",
                "--add",
                "--cacheinfo",
                &info,
            ],
        );
    }
    GitOid::from_hex(&git(w.path(), &["write-tree"])).unwrap()
}

/// Whether git, with both of its checkout protections on, accepts `path` with `mode`
/// into an index of its own (so earlier probes cannot conflict with this one).
fn git_accepts(w: &Work, mode: &str, path: &str) -> bool {
    let oid = GitOid::for_blob(b"x").to_hex();
    let index = tempfile::NamedTempFile::new().unwrap();
    std::fs::remove_file(index.path()).unwrap();
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
        .env("GIT_INDEX_FILE", index.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stderr(Stdio::null())
        .status()
        .unwrap();
    status.success()
}

/// What cuenca says about one entry at `path`: Ok, or the error.
fn cuenca(mode: &str, path: &str) -> Result<(), TreeError> {
    let path = RepoPath::new(path)?;
    let entry = if mode == "120000" {
        Entry::symlink(b"x")
    } else {
        Entry::file(b"x")
    };
    Tree::from_entries([(path, entry)]).map(|_| ())
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

/// A deterministic stream of path names built from the characters git's `.git` and
/// `.gitmodules` rules care about.
fn fuzz_paths(n: usize, seed: u64) -> Vec<String> {
    let pieces = [
        ".",
        "g",
        "G",
        "i",
        "I",
        "t",
        "T",
        "~",
        "1",
        "4",
        "9",
        " ",
        ":",
        "\u{200C}",
        "\u{FEFF}",
        "modules",
        "MODULES",
        "gi7eba",
        "gitmod",
        "attributes",
        "gi7d29",
        "/",
        "a",
        "x",
    ];
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    (0..n)
        .map(|_| {
            let len = 1 + next() % 7;
            (0..len)
                .map(|_| pieces[next() % pieces.len()])
                .collect::<String>()
        })
        .filter(|p| RepoPath::new(p.clone()).is_ok() || !p.contains('/'))
        .collect()
}

/// Path names on which cuenca claims exact parity with git's `verify_path` for files.
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

/// A seeded fuzz over files and symlinks, multi-component paths included: whatever git
/// refuses, cuenca refuses; whatever cuenca refuses that git accepts is one of its
/// deliberate extra rules (no `.gitmodules` anywhere, `.gitattributes` only as a file,
/// which `git fsck` enforces even where `verify_path` does not).
#[test]
fn path_rules_cover_gits_own_under_fuzz() {
    let w = Work::new();
    let mut problems = Vec::new();
    let paths = fuzz_paths(700, 0x5eed);
    for mode in ["100644", "120000"] {
        for path in &paths {
            let theirs = git_accepts(&w, mode, path);
            match (cuenca(mode, path), theirs) {
                (Ok(()), false) => {
                    problems.push(format!("{mode} {path:?}: cuenca accepts, git refuses"))
                }
                (Err(TreeError::Gitmodules(_) | TreeError::GitattributesNotAFile(_)), true) => {}
                (Err(e), true) => problems.push(format!(
                    "{mode} {path:?}: cuenca refuses ({e}), git accepts"
                )),
                _ => {}
            }
        }
    }
    assert!(
        paths.len() > 400,
        "the fuzz produced only {} paths",
        paths.len()
    );
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// `git fsck` as the oracle for trees git's index would accept but its object checks
/// reject (as GitHub does on push): every one must be refused when read.
#[test]
fn trees_git_fsck_rejects_are_refused() {
    let long_line = format!("{}\n", "a".repeat(3000));
    let cases: [(&str, &[IndexEntry]); 4] = [
        (
            "a .gitmodules directory",
            &[("100644", ".gitmodules/x", b"x")],
        ),
        (
            "a .gitattributes directory",
            &[("100644", "a/.gitattributes/x", b"x")],
        ),
        (
            "a .gitattributes with a 3000-byte line",
            &[("100644", ".gitattributes", long_line.as_bytes())],
        ),
        (
            "a .gitmodules file",
            &[(
                "100644",
                ".gitmodules",
                b"[submodule \"x\"]\n\tpath = x\n\turl = -oProxy\n",
            )],
        ),
    ];
    for (what, entries) in cases {
        let w = Work::new();
        let tree = tree_from_index(&w, entries);
        let commit = git(
            w.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit-tree",
                &tree.to_hex(),
                "-m",
                "x",
            ],
        );
        git(w.path(), &["update-ref", "refs/heads/main", &commit]);
        let fsck = Command::new("git")
            .arg("-C")
            .arg(w.path())
            .args(["fsck", "--no-dangling"])
            .output()
            .unwrap();
        assert!(
            !fsck.status.success(),
            "{what}: expected git fsck to reject it"
        );
        assert!(
            Repo::open(w.path()).read_tree(&tree.to_hex()).is_err(),
            "{what}: cuenca read it"
        );
    }
}

/// On a case-insensitive APFS volume, the filesystem itself is the oracle. For every
/// BMP code point and the cased astral blocks, create `x<c>` and probe its variants
/// (upper, lower, upper-then-lower, NFC, NFD): APFS resolves a variant exactly when
/// cuenca refuses to hold both names.
#[cfg(target_os = "macos")]
#[test]
fn fold_collisions_match_apfs_across_unicode() {
    use unicode_normalization::UnicodeNormalization;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("probe"), "").unwrap();
    if !dir.path().join("PROBE").exists() {
        eprintln!("skipped: the temp volume is case-sensitive");
        return;
    }
    let astral = [
        0x10400..0x10450,
        0x104B0..0x10500,
        0x10570..0x105C0,
        0x10C80..0x10D00,
        0x118A0..0x11900,
        0x16E40..0x16EA0,
        0x1E900..0x1E960,
    ];
    let chars = (0x21u32..0x10000)
        .chain(astral.into_iter().flatten())
        .filter_map(char::from_u32);
    let mut disagreements = Vec::new();
    let mut probed = 0;
    for c in chars {
        let name = format!("x{c}");
        if RepoPath::new(name.clone()).is_err() {
            continue;
        }
        let file = dir.path().join(&name);
        if std::fs::write(&file, "").is_err() {
            continue;
        }
        let variants = [
            name.to_uppercase(),
            name.to_lowercase(),
            name.to_uppercase().to_lowercase(),
            name.to_lowercase().to_uppercase(),
            name.nfc().collect::<String>(),
            name.nfd().collect::<String>(),
        ];
        for v in variants.iter().filter(|v| **v != name) {
            if RepoPath::new(v.clone()).is_err() {
                continue;
            }
            probed += 1;
            let apfs = dir.path().join(v).exists();
            let ours = matches!(
                Tree::from_entries([
                    (RepoPath::new(name.clone()).unwrap(), Entry::file(b"1")),
                    (RepoPath::new(v.clone()).unwrap(), Entry::file(b"2"))
                ]),
                Err(TreeError::Collision { .. })
            );
            if apfs != ours {
                disagreements.push(format!(
                    "{name:?}/{v:?} (U+{:04X}): APFS {apfs}, cuenca {ours}",
                    c as u32
                ));
            }
        }
        std::fs::remove_file(&file).unwrap();
    }
    assert!(probed > 2000, "only {probed} pairs probed");
    assert!(
        disagreements.is_empty(),
        "{} disagreements:\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

#[test]
fn a_case_collision_in_git_is_refused() {
    let w = Work::new();
    let t = tree_from_index(
        &w,
        &[("100644", "README.md", b"a"), ("100644", "readme.md", b"b")],
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
        &[
            ("100644", "caf\u{e9}", b"a"),
            ("100644", "cafe\u{301}", b"b"),
        ],
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
    let commit = b"1111111111111111111111111111111111111111";
    let t = tree_from_index(
        &w,
        &[("160000", "vendor/sub", commit), ("100644", "a.txt", b"a")],
    );
    let err = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::UnsupportedMode(_))),
        "{err}"
    );
}

#[test]
fn an_empty_directory_is_refused_by_its_own_name() {
    let w = Work::new();
    let empty = mktree(&w, "", false);
    assert_eq!(empty, GitOid::empty_tree().to_hex());
    let a = blob(&w, b"a");
    let inner = mktree(
        &w,
        &format!("040000 tree {empty}\tleaf\n100644 blob {a}\tkeep\n"),
        false,
    );
    let root = mktree(
        &w,
        &format!("040000 tree {inner}\touter\n100644 blob {a}\ta.txt\n"),
        false,
    );
    let err = Repo::open(w.path()).read_tree(&root).unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::EmptyDirectory(ref d)) if d == "outer/leaf"),
        "{err}"
    );
}

#[test]
fn replace_refs_cannot_swap_a_blob() {
    let w = Work::new();
    let t = tree_from_index(&w, &[("100644", "a.txt", b"original")]);
    let evil = blob(&w, b"replaced");
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
fn writing_refuses_content_that_does_not_match_its_id() {
    let tree = Tree::from_entries([(RepoPath::new("a").unwrap(), Entry::file(b"real"))]).unwrap();
    let mut blobs = Blobs::new();
    blobs.insert(GitOid::for_blob(b"real"), b"forged".to_vec());
    let w = Work::new();
    let err = Repo::open(w.path()).write_tree(&tree, &blobs).unwrap_err();
    assert!(matches!(err, GitError::Corrupt(_)), "{err}");
    let forged = GitOid::for_blob(b"forged").to_hex();
    let stored = Command::new("git")
        .arg("-C")
        .arg(w.path())
        .args(["cat-file", "-e", &forged])
        .status()
        .unwrap();
    assert!(
        !stored.success(),
        "the forged content was written before being refused"
    );
    let err = Repo::open(w.path())
        .write_tree(&tree, &Default::default())
        .unwrap_err();
    assert!(matches!(err, GitError::NoContent(_)), "{err}");
}

/// The repository already holds a blob with a 3000-byte line; naming it at a
/// `.gitattributes` path must still be refused, which means reading it back.
#[test]
fn writing_checks_gitattributes_the_repository_already_has() {
    let w = Work::new();
    let long = format!("{}\n", "a".repeat(3000));
    blob(&w, long.as_bytes());
    let tree = Tree::from_entries([(
        RepoPath::new("src/.gitattributes").unwrap(),
        Entry::file(long.as_bytes()),
    )])
    .unwrap();
    let err = Repo::open(w.path())
        .write_tree(&tree, &Blobs::new())
        .unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::GitattributesContent { .. })),
        "{err}"
    );
}

#[test]
fn a_sha256_repository_is_refused_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "--object-format=sha256"]);
    let count = || git(dir.path(), &["count-objects"]);
    let before = count();
    let tree = Tree::from_entries([(RepoPath::new("a").unwrap(), Entry::file(b"a"))]).unwrap();
    let mut blobs = Blobs::new();
    blobs.insert(GitOid::for_blob(b"a"), b"a".to_vec());
    let err = Repo::open(dir.path())
        .write_tree(&tree, &blobs)
        .unwrap_err();
    assert!(
        matches!(err, GitError::UnsupportedObjectFormat(ref f) if f == "sha256"),
        "{err}"
    );
    assert_eq!(count(), before, "an object was written");
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

#[test]
fn reading_blobs_keeps_to_its_memory_limits() {
    let w = Work::new();
    let oids: Vec<GitOid> = (0..4)
        .map(|i| GitOid::from_hex(&blob(&w, &vec![b'a' + i; 1000])).unwrap())
        .collect();
    let repo = Repo::open(w.path()).with_memory_limits(1000, 2500);
    assert!(matches!(
        repo.read_blobs(oids.clone()),
        Err(GitError::TotalTooLarge { limit: 2500 })
    ));
    let small = Repo::open(w.path()).with_memory_limits(999, u64::MAX);
    assert!(matches!(
        small.read_blobs(oids.clone()),
        Err(GitError::TooLarge { .. })
    ));
    assert_eq!(Repo::open(w.path()).read_blobs(oids).unwrap().len(), 4);
}

/// A real `blob:none` partial clone of a local repository: reading must report the blob
/// missing and must not fetch it, so the clone still lacks it afterwards; writing into
/// it is refused.
#[test]
fn a_partial_clone_is_not_fetched_from_or_written_into() {
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

    let tree = Tree::from_entries([(RepoPath::new("b").unwrap(), Entry::file(b"b"))]).unwrap();
    let mut blobs = Blobs::new();
    blobs.insert(GitOid::for_blob(b"b"), b"b".to_vec());
    assert!(matches!(
        Repo::open(&clone).write_tree(&tree, &blobs),
        Err(GitError::PartialClone)
    ));
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
    let evil = blob(&w, b"evil\n");
    let objects = w.path().join(".git/objects");
    let from = objects.join(&evil[..2]).join(&evil[2..]);
    let to = objects.join(&real.to_hex()[..2]).join(&real.to_hex()[2..]);
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::copy(from, to).unwrap();
    let repo = Repo::open(w.path());
    assert!(matches!(repo.read_tree(&t), Err(GitError::Corrupt(o)) if o == real));
    assert!(matches!(repo.read_blobs([real]), Err(GitError::Corrupt(o)) if o == real));
    // Writing a tree that names it must not trust the object store either.
    let tree = Tree::from_entries([(RepoPath::new("a.txt").unwrap(), Entry::File(real))]).unwrap();
    let err = repo.write_tree(&tree, &Blobs::new()).unwrap_err();
    assert!(matches!(err, GitError::Corrupt(o) if o == real), "{err}");
}

/// Strata B and C's round-3 attack: the repository holds a blob with a 3000-byte line;
/// the tree names it at a `.gitattributes` path, and the caller sends benign bytes for
/// that id. The repository's own bytes are what would land, so they are what is checked.
#[test]
fn forged_bytes_cannot_vouch_for_a_gitattributes_the_repository_has() {
    let w = Work::new();
    let long = format!("{}\n", "a".repeat(3000));
    let oid = GitOid::from_hex(&blob(&w, long.as_bytes())).unwrap();
    let tree =
        Tree::from_entries([(RepoPath::new(".gitattributes").unwrap(), Entry::File(oid))]).unwrap();
    let mut forged = Blobs::new();
    forged.insert(oid, b"* text\n".to_vec());
    let err = Repo::open(w.path()).write_tree(&tree, &forged).unwrap_err();
    // The forged bytes are refused for not being what the id names, before anything
    // is written; with no bytes sent, the read-back refuses the long line instead
    // (`writing_checks_gitattributes_the_repository_already_has`).
    assert!(matches!(err, GitError::Corrupt(o) if o == oid), "{err}");
}

/// Stratum C's round-3 cost attack: one large `.gitattributes` blob named at many
/// paths is loaded and checked once, not once per path. The write goes into the
/// repository that already holds the trees, so it times the checks alone: building
/// 10,000 new directories costs one `git mktree` each (about 90 s), which is the
/// documented price of having git lay out every tree.
#[test]
fn many_gitattributes_paths_over_one_blob_are_checked_once() {
    let w = Work::new();
    let content = "* text\n".repeat(400_000);
    let oid = blob(&w, content.as_bytes());
    let index: String = (0..10_000)
        .map(|i| format!("100644 {oid}\td{i}/.gitattributes\n"))
        .collect();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(w.path())
        .args(["update-index", "--index-info"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(index.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    let root = git(w.path(), &["write-tree"]);
    let repo = Repo::open(w.path());
    let started = std::time::Instant::now();
    let tree = repo.read_tree(&root).unwrap();
    let read = started.elapsed();
    let started = std::time::Instant::now();
    repo.write_tree(&tree, &Blobs::new()).unwrap();
    let wrote = started.elapsed();
    assert!(
        read.as_secs() < 5 && wrote.as_secs() < 5,
        "read {read:?}, write {wrote:?}"
    );
}

#[test]
fn a_blob_id_that_is_a_tree_in_the_repository_is_refused() {
    let w = Work::new();
    let a = blob(&w, b"a");
    let sub = GitOid::from_hex(&mktree(&w, &format!("100644 blob {a}\tx\n"), false)).unwrap();
    let tree = Tree::from_entries([(RepoPath::new("f").unwrap(), Entry::File(sub))]).unwrap();
    let err = Repo::open(w.path())
        .write_tree(&tree, &Blobs::new())
        .unwrap_err();
    assert!(matches!(err, GitError::WrongType(o) if o == sub), "{err}");
}

/// Plant a loose object: copy the object file of `from` to where `to` would live.
fn plant(objects: &Path, from: &str, to: &str) {
    let target = objects.join(&to[..2]).join(&to[2..]);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&target);
    std::fs::copy(objects.join(&from[..2]).join(&from[2..]), target).unwrap();
}

/// Strata A, B and C's round-4 attack: the repository already holds, under the id of
/// subtree `d`, the bytes of a different tree pointing at other content. The write must
/// not certify it.
#[test]
fn a_corrupt_tree_already_in_the_store_fails_the_write() {
    let w = Work::new();
    let real = blob(&w, b"real\n");
    let evil = blob(&w, b"evil\n");
    let good_sub = mktree(&w, &format!("100644 blob {real}\ta.txt\n"), false);
    let evil_sub = mktree(&w, &format!("100644 blob {evil}\ta.txt\n"), false);
    std::fs::remove_file(
        w.path()
            .join(".git/objects")
            .join(&good_sub[..2])
            .join(&good_sub[2..]),
    )
    .unwrap();
    plant(&w.path().join(".git/objects"), &evil_sub, &good_sub);
    let tree =
        Tree::from_entries([(RepoPath::new("d/a.txt").unwrap(), Entry::file(b"real\n"))]).unwrap();
    let err = Repo::open(w.path())
        .write_tree(&tree, &Blobs::new())
        .unwrap_err();
    assert!(
        matches!(
            err,
            GitError::StoreMismatch(_) | GitError::NonCanonical { .. }
        ),
        "{err}"
    );
}

/// The same substitution supplied through `objects/info/alternates`: a second store can
/// hold a different copy under the same id, so a repository borrowing objects is refused.
#[test]
fn a_repository_with_alternates_is_refused() {
    let other = Work::new();
    let real = blob(&other, b"real\n");
    let evil = blob(&other, b"evil\n");
    let good_sub = mktree(&other, &format!("100644 blob {real}\ta.txt\n"), false);
    let evil_sub = mktree(&other, &format!("100644 blob {evil}\ta.txt\n"), false);
    let objects = other.path().join(".git/objects");
    std::fs::remove_file(objects.join(&good_sub[..2]).join(&good_sub[2..])).unwrap();
    plant(&objects, &evil_sub, &good_sub);
    let w = Work::new();
    std::fs::write(
        w.path().join(".git/objects/info/alternates"),
        format!("{}\n", objects.display()),
    )
    .unwrap();
    let tree =
        Tree::from_entries([(RepoPath::new("d/a.txt").unwrap(), Entry::file(b"real\n"))]).unwrap();
    let err = Repo::open(w.path())
        .write_tree(&tree, &Blobs::new())
        .unwrap_err();
    assert!(matches!(err, GitError::Alternates), "{err}");
    let err = Repo::open(w.path()).read_tree("HEAD").unwrap_err();
    assert!(matches!(err, GitError::Alternates), "{err}");
}

/// The most common hostile case: an agent sends a new `.gitattributes` with a line git's
/// fsck rejects. It is refused before anything is stored.
#[test]
fn a_new_long_line_gitattributes_is_refused_before_anything_is_written() {
    let w = Work::new();
    let long = format!("{}\n", "a".repeat(3000));
    let tree = Tree::from_entries([
        (
            RepoPath::new("benign.txt").unwrap(),
            Entry::file(b"benign\n"),
        ),
        (
            RepoPath::new(".gitattributes").unwrap(),
            Entry::file(long.as_bytes()),
        ),
    ])
    .unwrap();
    let mut blobs = Blobs::new();
    blobs.insert(GitOid::for_blob(b"benign\n"), b"benign\n".to_vec());
    blobs.insert(GitOid::for_blob(long.as_bytes()), long.into_bytes());
    let before = git(w.path(), &["count-objects"]);
    let err = Repo::open(w.path()).write_tree(&tree, &blobs).unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::GitattributesContent { .. })),
        "{err}"
    );
    assert_eq!(
        git(w.path(), &["count-objects"]),
        before,
        "something was written"
    );
}

/// Bytes the caller sends are checked against their ids even when the repository
/// already has the blob, so "the bridge verifies what it is sent" holds without
/// exception.
#[test]
fn forged_bytes_for_a_present_blob_are_refused() {
    let w = Work::new();
    blob(&w, b"real\n");
    let tree = Tree::from_entries([(RepoPath::new("a").unwrap(), Entry::file(b"real\n"))]).unwrap();
    let mut forged = Blobs::new();
    forged.insert(GitOid::for_blob(b"real\n"), b"forged\n".to_vec());
    let err = Repo::open(w.path()).write_tree(&tree, &forged).unwrap_err();
    assert!(matches!(err, GitError::Corrupt(_)), "{err}");
}

#[test]
fn a_symlink_target_a_mac_cannot_create_is_refused() {
    let w = Work::new();
    let target = "t/".repeat(600);
    let t = tree_from_index(&w, &[("120000", "link", target.as_bytes())]);
    let err = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::SymlinkTarget { .. })),
        "{err}"
    );
    let tree = Tree::from_entries([(
        RepoPath::new("link").unwrap(),
        Entry::symlink(target.as_bytes()),
    )])
    .unwrap();
    let mut blobs = Blobs::new();
    blobs.insert(GitOid::for_blob(target.as_bytes()), target.into_bytes());
    let fresh = Work::new();
    let before = git(fresh.path(), &["count-objects"]);
    let err = Repo::open(fresh.path())
        .write_tree(&tree, &blobs)
        .unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::SymlinkTarget { .. })),
        "{err}"
    );
    assert_eq!(
        git(fresh.path(), &["count-objects"]),
        before,
        "something was written"
    );
}

#[test]
fn reading_a_tree_id_as_a_blob_is_refused() {
    let w = Work::new();
    let a = blob(&w, b"a");
    let sub = GitOid::from_hex(&mktree(&w, &format!("100644 blob {a}\tx\n"), false)).unwrap();
    let err = Repo::open(w.path()).read_blobs([sub]).unwrap_err();
    assert!(matches!(err, GitError::WrongType(o) if o == sub), "{err}");
}

/// Store a tree object with exactly these bytes, bypassing git's own checks.
fn literal_tree(w: &Work, body: &[u8]) -> String {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(w.path())
        .args(["hash-object", "-t", "tree", "--literally", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(body).unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_string()
}

fn raw_entry(mode: &str, name: &str, oid: &GitOid) -> Vec<u8> {
    let mut e = format!("{mode} {name}\0").into_bytes();
    e.extend_from_slice(oid.as_bytes());
    e
}

/// Stratum B's round-5 attack: substitutes that git's listing normalises back to the
/// canonical tree (a legacy mode, reversed entry order, a zero-padded mode, a name
/// containing `/`), planted under the canonical id in the repository's own store and
/// through an alternate. Reading and writing must both refuse every one.
#[test]
fn substitutes_the_listing_normalises_are_refused() {
    let (a, b, x) = (
        GitOid::for_blob(b"A\n"),
        GitOid::for_blob(b"B\n"),
        GitOid::for_blob(b"X\n"),
    );
    let flat = Tree::from_entries([
        (RepoPath::new("d/a").unwrap(), Entry::File(a)),
        (RepoPath::new("d/b").unwrap(), Entry::File(b)),
        (RepoPath::new("top").unwrap(), Entry::file(b"T\n")),
    ])
    .unwrap();
    let nested = Tree::from_entries([
        (RepoPath::new("d/a/x").unwrap(), Entry::File(x)),
        (RepoPath::new("d/b").unwrap(), Entry::File(b)),
        (RepoPath::new("top").unwrap(), Entry::file(b"T\n")),
    ])
    .unwrap();
    let variants: [(&str, &Tree, Vec<u8>); 4] = [
        (
            "legacy mode",
            &flat,
            [raw_entry("100664", "a", &a), raw_entry("100644", "b", &b)].concat(),
        ),
        (
            "reversed order",
            &flat,
            [raw_entry("100644", "b", &b), raw_entry("100644", "a", &a)].concat(),
        ),
        (
            "zero-padded mode",
            &flat,
            [raw_entry("0100644", "a", &a), raw_entry("100644", "b", &b)].concat(),
        ),
        (
            "a name with a slash",
            &nested,
            [raw_entry("100644", "a/x", &x), raw_entry("100644", "b", &b)].concat(),
        ),
    ];
    for (what, tree, body) in &variants {
        for through_alternate in [false, true] {
            let store = Work::new();
            for content in [&b"A\n"[..], b"B\n", b"X\n", b"T\n"] {
                blob(&store, content);
            }
            let sub = tree
                .git_objects()
                .iter()
                .rev()
                .nth(1)
                .map(|t| t.oid)
                .unwrap()
                .to_hex();
            let forged = literal_tree(&store, body);
            let objects = store.path().join(".git/objects");
            plant(&objects, &forged, &sub);
            let w = if through_alternate {
                let w = Work::new();
                std::fs::write(
                    w.path().join(".git/objects/info/alternates"),
                    format!("{}\n", objects.display()),
                )
                .unwrap();
                w
            } else {
                store
            };
            let repo = Repo::open(w.path());
            let refused = |e: &GitError| {
                if through_alternate {
                    matches!(e, GitError::Alternates)
                } else {
                    matches!(e, GitError::StoreMismatch(_))
                }
            };
            let err = repo.write_tree(tree, &Blobs::new()).unwrap_err();
            assert!(
                refused(&err),
                "{what} (alternate: {through_alternate}): write gave {err}"
            );
            let err = repo.read_tree(&tree.git_tree_id().to_hex()).unwrap_err();
            assert!(
                refused(&err),
                "{what} (alternate: {through_alternate}): read gave {err}"
            );
        }
    }
}

/// Read directly (not through a write) a parent of a corrupt subtree.
#[test]
fn reading_a_tree_with_a_corrupt_subtree_is_refused() {
    let w = Work::new();
    let real = blob(&w, b"real\n");
    let evil = blob(&w, b"evil\n");
    let good_sub = mktree(&w, &format!("100644 blob {real}\ta.txt\n"), false);
    let evil_sub = mktree(&w, &format!("100644 blob {evil}\ta.txt\n"), false);
    let root = mktree(&w, &format!("040000 tree {good_sub}\td\n"), false);
    let objects = w.path().join(".git/objects");
    std::fs::remove_file(objects.join(&good_sub[..2]).join(&good_sub[2..])).unwrap();
    plant(&objects, &evil_sub, &good_sub);
    let err = Repo::open(w.path()).read_tree(&root).unwrap_err();
    assert!(
        matches!(
            err,
            GitError::StoreMismatch(_) | GitError::NonCanonical { .. }
        ),
        "{err}"
    );
}

/// Strata A and C's round-5 attack: a symlink target `symlink(2)` cannot hold (empty, or
/// with a NUL byte) is refused on read, and on write before anything is stored.
#[test]
fn symlink_targets_no_checkout_can_hold_are_refused() {
    for target in [&b""[..], b"safe\0evil"] {
        let w = Work::new();
        let t = tree_from_index(&w, &[("120000", "link", target)]);
        let err = Repo::open(w.path()).read_tree(&t.to_hex()).unwrap_err();
        assert!(
            matches!(err, GitError::Tree(TreeError::SymlinkTarget { .. })),
            "{target:?}: {err}"
        );

        let fresh = Work::new();
        let tree =
            Tree::from_entries([(RepoPath::new("link").unwrap(), Entry::symlink(target))]).unwrap();
        let mut blobs = Blobs::new();
        blobs.insert(GitOid::for_blob(target), target.to_vec());
        let before = git(fresh.path(), &["count-objects"]);
        let err = Repo::open(fresh.path())
            .write_tree(&tree, &blobs)
            .unwrap_err();
        assert!(
            matches!(err, GitError::Tree(TreeError::SymlinkTarget { .. })),
            "{target:?}: {err}"
        );
        assert_eq!(
            git(fresh.path(), &["count-objects"]),
            before,
            "{target:?}: something was written"
        );
    }
}

/// An honestly hashed tree with a legacy `100664` mode (old git history) is refused,
/// and named as non-canonical rather than as tampering.
#[test]
fn a_legacy_mode_tree_is_named_non_canonical() {
    let w = Work::new();
    let a = GitOid::from_hex(&blob(&w, b"a\n")).unwrap();
    let legacy = literal_tree(&w, &raw_entry("100664", "a", &a));
    let err = Repo::open(w.path()).read_tree(&legacy).unwrap_err();
    assert!(matches!(err, GitError::NonCanonical { .. }), "{err}");
}

/// Stratum B's round-6 case: the store already has the empty blob, and the caller
/// sends nothing for it; a symlink to it is still refused before anything is written.
#[test]
fn a_present_blob_failing_a_content_rule_stores_nothing() {
    let w = Work::new();
    blob(&w, b"");
    let tree = Tree::from_entries([(RepoPath::new("l").unwrap(), Entry::symlink(b""))]).unwrap();
    let before = git(w.path(), &["count-objects"]);
    let err = Repo::open(w.path())
        .write_tree(&tree, &Blobs::new())
        .unwrap_err();
    assert!(
        matches!(err, GitError::Tree(TreeError::SymlinkTarget { .. })),
        "{err}"
    );
    assert_eq!(
        git(w.path(), &["count-objects"]),
        before,
        "something was written"
    );
}

/// Stratum C's round-7 case: git honours an alternates line of only spaces as a relative
/// path, so any content at all in the file is refused; an empty file is not a second
/// store. Every reading method checks, not only read_tree and write_tree.
#[test]
fn any_alternates_content_is_refused_by_every_reader() {
    let w = Work::new();
    let a = GitOid::from_hex(&blob(&w, b"a\n")).unwrap();
    let t = tree_from_index(&w, &[("100644", "a", b"a\n")]);
    let file = w.path().join(".git/objects/info/alternates");
    std::fs::write(&file, "").unwrap();
    assert!(
        Repo::open(w.path()).read_tree(&t.to_hex()).is_ok(),
        "an empty alternates file is no second store"
    );
    std::fs::write(&file, " \n").unwrap();
    let repo = Repo::open(w.path());
    assert!(matches!(
        repo.read_tree(&t.to_hex()),
        Err(GitError::Alternates)
    ));
    assert!(matches!(
        repo.resolve_tree(&t.to_hex()),
        Err(GitError::Alternates)
    ));
    assert!(matches!(repo.read_blobs([a]), Err(GitError::Alternates)));
}
