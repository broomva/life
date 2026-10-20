//! The spec's phase-2 acceptance row: every entry kind round-trips git → changeset → git
//! with identical git tree ids, in a 40-case matrix of 8 operations × 5 placements.
//!
//! For each case, a real working directory is changed through the filesystem and git
//! reads it (the oracle). Independently, cuenca builds the same change in-process from a
//! model. The case passes only if all of these hold:
//!
//! 1. cuenca's tree id for the base and for the result equal git's;
//! 2. reading git's base tree gives exactly the modelled tree;
//! 3. the changeset survives JSON and applies to the tree read from git;
//! 4. writing the result into a fresh repository gives git's tree id, with every
//!    directory built by `git mktree` agreeing with cuenca;
//! 5. a checkout of that tree puts the modelled files on disk: kinds, executable bits,
//!    contents and link targets;
//! 6. git's own diff of its two trees (`git diff-tree`) lists exactly the changeset's
//!    ops, and cuenca's diff of the trees read from git gives the same changeset.

#![cfg(unix)]

mod common;

use common::{Kind, Work};
use cuenca_tree::git::Repo;
use cuenca_tree::{Changeset, GitOid, Op};

struct Placement {
    name: &'static str,
    dir: &'static str,
}

const PLACEMENTS: [Placement; 5] = [
    Placement {
        name: "root",
        dir: "",
    },
    Placement {
        name: "deep",
        dir: "d1/d2/d3/d4/",
    },
    Placement {
        name: "quoted",
        dir: "sp ace/tab\tq'uo\"te/",
    },
    Placement {
        name: "unicode-dir",
        dir: "nl\nx/emoji-🦀/",
    },
    Placement {
        name: "binary",
        dir: "bin/",
    },
];

const OPS: [&str; 8] = [
    "file",
    "executable",
    "symlink",
    "directory",
    "delete",
    "kind-change",
    "rename",
    "non-ascii",
];

/// Content for new files in placement `i`: empty at the root, 1 MiB of binary with NUL
/// bytes in `bin/`, short text elsewhere.
fn content(i: usize, seed: &str) -> Vec<u8> {
    match i {
        0 => Vec::new(),
        4 => {
            let mut state: u32 = 0x9e37_79b9 ^ seed.len() as u32;
            (0..1 << 20)
                .map(|n| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    if n % 7 == 0 { 0 } else { (state >> 24) as u8 }
                })
                .collect()
        }
        _ => format!("{seed}\n").into_bytes(),
    }
}

fn base(w: &mut Work) {
    for (i, p) in PLACEMENTS.iter().enumerate() {
        let d = p.dir;
        w.write(
            &format!("{d}keep.txt"),
            Kind::File,
            format!("keep {d}\n").as_bytes(),
        );
        for sibling in ["x.y", "x-y", "x0"] {
            w.write(
                &format!("{d}{sibling}"),
                Kind::File,
                format!("{sibling} {d}\n").as_bytes(),
            );
        }
        let old = if i == 4 {
            content(4, "old")[..64 * 1024].to_vec()
        } else {
            format!("old {d}\n").into_bytes()
        };
        w.write(&format!("{d}old.txt"), Kind::File, &old);
        w.write(
            &format!("{d}tool.sh"),
            Kind::Exec,
            format!("#!/bin/sh\necho {d}\n").as_bytes(),
        );
        w.write(&format!("{d}link"), Kind::Link, b"keep.txt");
    }
}

fn act(w: &mut Work, op: &str, i: usize) {
    let d = PLACEMENTS[i].dir;
    let at = |name: &str| format!("{d}{name}");
    let c = content(i, op);
    match op {
        "file" => w.write(&at("x"), Kind::File, &c),
        "executable" => w.write(&at("x"), Kind::Exec, &c),
        "symlink" => {
            let target = [
                "keep.txt",
                "../../../../keep.txt",
                "/etc/hosts",
                "ñandú/日本",
                "does/not/exist",
            ][i];
            w.write(&at("x"), Kind::Link, target.as_bytes());
        }
        "directory" => {
            w.write(&at("x/inner.txt"), Kind::File, &c);
            w.write(&at("x/sub/deep.txt"), Kind::File, &content(i, "deep"));
        }
        "delete" => match i {
            0 | 4 => w.remove(&at("old.txt")),
            1 => w.remove(&at("tool.sh")),
            2 => w.remove(&at("link")),
            _ => {
                let all: Vec<String> = w
                    .model
                    .keys()
                    .filter(|k| k.starts_with(d))
                    .cloned()
                    .collect();
                for path in all {
                    w.remove(&path);
                }
            }
        },
        "kind-change" => match i {
            0 => w.chmod(&at("old.txt"), true),
            1 => w.chmod(&at("tool.sh"), false),
            2 => w.write(&at("old.txt"), Kind::Link, b"keep.txt"),
            3 => w.write(&at("link"), Kind::File, &c),
            _ => {
                w.remove(&at("old.txt"));
                w.write(&at("old.txt/inner.bin"), Kind::File, &c);
            }
        },
        "rename" => match i {
            0 => w.rename(&at("old.txt"), &at("renamed.txt")),
            1 => w.rename(&at("old.txt"), "moved-from-deep.txt"),
            2 => w.rename(&at("tool.sh"), &at("tool-renamed.sh")),
            3 => w.rename(&at("link"), &at("link2")),
            _ => w.rename(&at("keep.txt"), &at("KEEP-renamed.txt")),
        },
        "non-ascii" => w.write(&at("ñandú-日本語-é-🦀.txt"), Kind::File, &c),
        other => unreachable!("unknown op {other}"),
    }
}

/// git's own diff of two trees, as (path, Some((mode, oid))) for a put and (path, None)
/// for a delete, sorted by path.
fn git_diff(
    dir: &std::path::Path,
    t0: GitOid,
    t1: GitOid,
) -> Vec<(String, Option<(String, String)>)> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "diff-tree",
            "-r",
            "--no-renames",
            "--raw",
            "-z",
            &t0.to_hex(),
            &t1.to_hex(),
        ])
        .output()
        .expect("git diff-tree runs");
    assert!(out.status.success());
    let mut fields = out.stdout.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut ops = Vec::new();
    while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
        let meta = String::from_utf8(meta.to_vec()).expect("ascii");
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let path = String::from_utf8(path.to_vec()).expect("utf-8");
        let put = (parts[4] != "D").then(|| (parts[1].to_string(), parts[3].to_string()));
        ops.push((path, put));
    }
    ops.sort();
    ops
}

fn ours_as_diff(change: &Changeset) -> Vec<(String, Option<(String, String)>)> {
    let mut ops: Vec<_> = change
        .ops
        .iter()
        .map(|op| match op {
            Op::Put { path, entry } => (
                path.to_string(),
                Some((entry.mode().as_str().to_string(), entry.oid().to_hex())),
            ),
            Op::Delete { path } => (path.to_string(), None),
            _ => unreachable!("only puts and deletes exist"),
        })
        .collect();
    ops.sort();
    ops
}

fn check<T: PartialEq + std::fmt::Debug>(what: &str, ours: T, git: T) -> Result<(), String> {
    if ours == git {
        Ok(())
    } else {
        Err(format!("{what}: cuenca {ours:?}, git {git:?}"))
    }
}

fn run_case(op: &str, i: usize) -> Result<(), String> {
    let mut w = Work::new();
    base(&mut w);
    let t0 = w.snapshot();
    let repo = Repo::open(w.path());
    let base_model = w.model_tree();
    check("base tree id", base_model.git_tree_id(), t0)?;
    let base_read = repo
        .read_tree(&t0.to_hex())
        .map_err(|e| format!("read base: {e}"))?;
    check("base read from git", &base_read, &base_model)?;

    act(&mut w, op, i);
    let t1 = w.snapshot();
    let target_model = w.model_tree();

    // 1. The changeset is built in-process from the model, then checked against git.
    let change = Changeset::between(&base_model, &target_model);
    if change.ops.is_empty() {
        return Err("the operation changed nothing".into());
    }
    check("result tree id", change.target, t1)?;

    // 3. Over the wire and onto what git holds.
    let json = serde_json::to_string(&change).map_err(|e| e.to_string())?;
    let change: Changeset = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let applied = change
        .apply(&base_read)
        .map_err(|e| format!("apply: {e}"))?;
    check("applied tree", &applied, &target_model)?;

    // 4. Into a fresh repository, every directory built by git mktree.
    let fresh = Work::new();
    let fresh_repo = Repo::open(fresh.path());
    let written = fresh_repo
        .write_tree(&applied, &w.model_blobs())
        .map_err(|e| format!("write: {e}"))?;
    check("written tree id", written, t1)?;
    let back = fresh_repo
        .read_tree(&t1.to_hex())
        .map_err(|e| format!("read back: {e}"))?;
    check("tree read back", &back, &applied)?;

    // 5. What git checks out is what the model holds.
    fresh.check_out_and_compare(t1, &w.model)?;

    // 6. git's diff lists exactly our ops, and our diff of git's trees is our changeset.
    check(
        "ops against git diff-tree",
        ours_as_diff(&change),
        git_diff(w.path(), t0, t1),
    )?;
    let target_read = repo
        .read_tree(&t1.to_hex())
        .map_err(|e| format!("read result: {e}"))?;
    check(
        "changeset from git's trees",
        Changeset::between(&base_read, &target_read),
        change,
    )
}

#[test]
fn forty_case_round_trip_matrix() {
    let mut failures = Vec::new();
    let mut cases = 0;
    for op in OPS {
        for (i, placement) in PLACEMENTS.iter().enumerate() {
            cases += 1;
            if let Err(why) = run_case(op, i) {
                failures.push(format!("{op} × {}: {why}", placement.name));
            }
        }
    }
    assert_eq!(cases, 40);
    println!(
        "round trip: {} of {cases} cases identical",
        cases - failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {cases} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
