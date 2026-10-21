mod common;

use common::*;
use cuenca_core::{ChangesetId, CoreError, Event, MAX_BLOB_BYTES, Store};
use cuenca_tree::{Changeset, Entry, GitOid, HashFailure, Op, Tree};

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn a_blob_is_stored_under_its_git_id_with_a_derived_sha256() {
    let (_d, store) = store();
    let content = b"hello\n";
    let oid = GitOid::for_blob(content);
    let info = store.ingest_blob(oid, 6, &content[..]).unwrap();
    assert_eq!(info.sha256, sha256(content));
    assert_eq!(info.size, 6);
    assert_eq!(store.blob_info(oid).unwrap(), Some(info));
    assert_eq!(store.read_blob(oid).unwrap(), content);
    // A second ingest reads nothing: an empty reader would otherwise fail the hash.
    assert_eq!(store.ingest_blob(oid, 6, &b""[..]).unwrap(), info);
}

#[test]
fn a_blob_that_does_not_hash_to_its_claim_is_refused_and_nothing_is_stored() {
    let (dir, store) = store();
    let claimed = GitOid::for_blob(b"what the sender said");
    let err = store.ingest_blob(claimed, 5, &b"other"[..]).unwrap_err();
    assert!(
        matches!(err, CoreError::OidMismatch { claimed: c, actual } if c == claimed && actual == GitOid::for_blob(b"other")),
        "{err}"
    );
    // Too few and too many bytes for the announced size.
    let oid = GitOid::for_blob(b"other");
    assert!(matches!(
        store.ingest_blob(oid, 6, &b"other"[..]).unwrap_err(),
        CoreError::Hash(HashFailure::WrongLength {
            expected: 6,
            seen: 5
        })
    ));
    assert!(matches!(
        store.ingest_blob(oid, 4, &b"other"[..]).unwrap_err(),
        CoreError::Hash(HashFailure::WrongLength {
            expected: 4,
            seen: 5
        })
    ));
    assert!(matches!(
        store
            .ingest_blob(oid, MAX_BLOB_BYTES + 1, &b"other"[..])
            .unwrap_err(),
        CoreError::TooLarge { .. }
    ));
    assert_eq!(store.blob_info(claimed).unwrap(), None);
    assert_eq!(store.blob_info(oid).unwrap(), None);
    assert!(matches!(
        store.read_blob(claimed),
        Err(CoreError::MissingBlob(_))
    ));
    assert_eq!(
        files_under(&dir.path().join("blobs")),
        Vec::<std::path::PathBuf>::new()
    );
}

#[test]
fn a_blob_damaged_on_disk_is_reported_not_returned() {
    let (dir, store) = store();
    let oid = ingest(&store, b"precious");
    let file = files_under(&dir.path().join("blobs/objects"));
    assert_eq!(file.len(), 1);
    std::fs::write(&file[0], b"precioux").unwrap();
    assert!(matches!(store.read_blob(oid), Err(CoreError::CorruptBlob(o)) if o == oid));
}

#[test]
fn locations_hold_any_tree_and_an_optional_remote() {
    let (_d, store) = store();
    let notes = tree(&store, &[("a.md", b"# a\n"), ("dir/b.md", b"b\n")]);
    store.add_location(&loc("notes"), None, &notes).unwrap();
    let repo = tree(&store, &[("README.md", b"hi\n")]);
    store
        .add_location(&loc("life"), Some("git@github.com:broomva/life.git"), &repo)
        .unwrap();

    assert_eq!(store.main_tree(&loc("notes")).unwrap(), notes);
    let life = store.location(&loc("life")).unwrap().unwrap();
    assert_eq!(
        life.remote.as_deref(),
        Some("git@github.com:broomva/life.git")
    );
    assert_eq!(life.main, repo.git_tree_id());
    assert_eq!(store.locations().unwrap().len(), 2);
    assert!(matches!(
        store.add_location(&loc("notes"), None, &notes),
        Err(CoreError::LocationExists(_))
    ));
    let long = "x".repeat(513);
    for bad in ["", "-uupload-pack=x", "a b", "a\nb", &long] {
        assert!(matches!(
            store.add_location(&loc("x"), Some(bad), &notes),
            Err(CoreError::BadRemote(_))
        ));
    }
    assert!(matches!(
        store.main_tree(&loc("nope")),
        Err(CoreError::UnknownLocation(_))
    ));
    assert_eq!(store.len().unwrap(), 2);
}

#[test]
fn a_tree_naming_an_uningested_blob_is_refused_and_the_log_is_unchanged() {
    let (_d, store) = store();
    let missing = Tree::from_entries([(path("x"), Entry::file(b"never sent"))]).unwrap();
    assert!(matches!(
        store.add_location(&loc("l"), None, &missing),
        Err(CoreError::MissingBlob(_))
    ));
    assert!(store.is_empty().unwrap());
    assert_eq!(store.location(&loc("l")).unwrap(), None);
}

#[test]
fn a_fork_moves_by_drafts_from_its_head() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    assert_eq!(store.fork_head(&fork("f")).unwrap(), main);

    let one = write(&store, &main, &[("a", b"2"), ("b/c", b"3")]);
    let p1 = store.publish_draft(&fork("f"), &one).unwrap();
    assert!(p1.fresh);
    assert_eq!(p1.id, ChangesetId::of(&one));
    let head = store.fork_head(&fork("f")).unwrap();
    assert_eq!(head.git_tree_id(), one.target);
    assert_eq!(store.tree(one.target).unwrap(), head);
    assert_eq!(store.changeset(p1.id).unwrap(), Some(one.clone()));

    // A publish from a stale base is refused and appends nothing.
    let len = store.len().unwrap();
    let stale = write(&store, &main, &[("a", b"9")]);
    assert!(matches!(
        store.publish_draft(&fork("f"), &stale),
        Err(CoreError::NotAtBase { .. })
    ));
    assert_eq!(store.len().unwrap(), len);

    // Main is untouched by drafts.
    assert_eq!(store.main_tree(&loc("l")).unwrap(), main);
    let f = store.fork(&fork("f")).unwrap().unwrap();
    assert_eq!(f.base, main.git_tree_id());
    assert_eq!(f.head_changeset, Some((p1.id, p1.offset)));
    assert!(matches!(
        store.create_fork(&fork("f"), &loc("l")),
        Err(CoreError::ForkExists(_))
    ));
    assert!(matches!(
        store.create_fork(&fork("g"), &loc("nope")),
        Err(CoreError::UnknownLocation(_))
    ));
}

#[test]
fn a_retried_publish_appends_nothing() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    let cs = write(&store, &main, &[("a", b"2")]);
    let first = store.publish_draft(&fork("f"), &cs).unwrap();
    let len = store.len().unwrap();
    let again = store.publish_draft(&fork("f"), &cs).unwrap();
    assert_eq!(
        (again.id, again.offset, again.fresh),
        (first.id, first.offset, false)
    );
    assert_eq!(store.len().unwrap(), len);
}

#[test]
fn a_draft_needs_every_blob_it_puts_and_passes_content_rules() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    let len = store.len().unwrap();

    let unsent = Changeset::new(&main, vec![Op::put(path("b"), Entry::file(b"unsent"))]).unwrap();
    assert!(matches!(
        store.publish_draft(&fork("f"), &unsent),
        Err(CoreError::MissingBlob(_))
    ));

    // A symlink whose target holds a NUL cannot be checked out; the rule reads the blob.
    let target = ingest(&store, b"bad\0target");
    let link = Changeset::new(&main, vec![Op::put(path("l"), Entry::Symlink(target))]).unwrap();
    assert!(matches!(
        store.publish_draft(&fork("f"), &link),
        Err(CoreError::Tree(_))
    ));
    // The same bytes as a regular file have no rule.
    let file = Changeset::new(&main, vec![Op::put(path("l"), Entry::File(target))]).unwrap();
    store.publish_draft(&fork("f"), &file).unwrap();
    assert_eq!(store.len().unwrap(), len + 1);
}

#[test]
fn main_advances_by_compare_and_swap() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    store.create_fork(&fork("g"), &loc("l")).unwrap();
    let f1 = write(&store, &main, &[("a", b"f")]);
    let g1 = write(&store, &main, &[("a", b"g")]);
    let pf = store.publish_draft(&fork("f"), &f1).unwrap();
    let pg = store.publish_draft(&fork("g"), &g1).unwrap();

    // Only the fork's head can move main.
    assert!(matches!(
        store.advance_main(&fork("g"), pf.id),
        Err(CoreError::NotHead { .. })
    ));
    let at = store.advance_main(&fork("f"), pf.id).unwrap();
    assert_eq!(store.main_tree(&loc("l")).unwrap().git_tree_id(), f1.target);
    assert_eq!(
        store
            .location(&loc("l"))
            .unwrap()
            .unwrap()
            .main_moved
            .map(|m| (m.changeset, m.fork, m.offset)),
        Some((pf.id, fork("f"), at))
    );
    // Another fork asking for the move f made is not a retry of it.
    assert!(matches!(
        store.advance_main(&fork("g"), pf.id),
        Err(CoreError::NotHead { .. })
    ));
    // Something else is appended; a retry still answers with the move's own offset.
    store.create_fork(&fork("h"), &loc("l")).unwrap();
    assert_eq!(store.fork(&fork("h")).unwrap().unwrap().base, f1.target);
    let len = store.len().unwrap();
    assert_eq!(store.advance_main(&fork("f"), pf.id).unwrap(), at);
    assert_eq!(store.len().unwrap(), len);
    // g was based on the old main: the swap fails.
    assert!(matches!(
        store.advance_main(&fork("g"), pg.id),
        Err(CoreError::NotAtBase { .. })
    ));
    assert!(matches!(
        store.advance_main(&fork("nope"), pg.id),
        Err(CoreError::UnknownFork(_))
    ));
    assert_eq!(store.len().unwrap(), len);
}

#[test]
fn a_landing_intent_names_the_head_and_a_retry_appends_nothing() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    let cs = write(&store, &main, &[("a", b"2")]);
    let p = store.publish_draft(&fork("f"), &cs).unwrap();
    let unknown = ChangesetId::of(&write(&store, &main, &[("z", b"z")]));
    assert!(matches!(
        store.record_landing_intent(&fork("f"), unknown),
        Err(CoreError::NotHead { .. })
    ));
    let at = store.record_landing_intent(&fork("f"), p.id).unwrap();
    store.create_fork(&fork("g"), &loc("l")).unwrap();
    let len = store.len().unwrap();
    assert_eq!(store.record_landing_intent(&fork("f"), p.id).unwrap(), at);
    assert_eq!(store.len().unwrap(), len);
    assert_eq!(
        store.fork(&fork("f")).unwrap().unwrap().landing_intent,
        Some((p.id, at))
    );
}

#[test]
fn a_publish_retried_after_a_newer_draft_is_refused_as_stale() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    let one = write(&store, &main, &[("a", b"2")]);
    store.publish_draft(&fork("f"), &one).unwrap();
    let after = store.fork_head(&fork("f")).unwrap();
    store
        .publish_draft(&fork("f"), &write(&store, &after, &[("a", b"3")]))
        .unwrap();
    assert!(matches!(
        store.publish_draft(&fork("f"), &one),
        Err(CoreError::NotAtBase { .. })
    ));
}

#[test]
fn state_survives_reopen_and_matches_a_replay_of_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let (cs, id) = {
        let store = Store::open(dir.path()).unwrap();
        let main = tree(&store, &[("a", b"1"), ("x/y/z", b"deep")]);
        store.add_location(&loc("l"), None, &main).unwrap();
        store.create_fork(&fork("f"), &loc("l")).unwrap();
        let cs = Changeset::new(
            &main,
            vec![
                Op::delete(path("x/y/z")),
                Op::put(
                    path("x/w"),
                    Entry::Executable(ingest(&store, b"#!/bin/sh\n")),
                ),
            ],
        )
        .unwrap();
        let p = store.publish_draft(&fork("f"), &cs).unwrap();
        store.record_landing_intent(&fork("f"), p.id).unwrap();
        store.advance_main(&fork("f"), p.id).unwrap();
        (cs, p.id)
    };
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.changeset(id).unwrap(), Some(cs.clone()));
    assert_eq!(store.main_tree(&loc("l")).unwrap().git_tree_id(), cs.target);
    let summary = store.replay_check().unwrap();
    assert_eq!(
        (
            summary.records,
            summary.locations,
            summary.forks,
            summary.changesets
        ),
        (5, 1, 1, 1)
    );
    let records = store.records(0, 100).unwrap();
    assert_eq!(records.len(), 5);
    assert!(
        records
            .iter()
            .enumerate()
            .all(|(i, r)| r.offset == i as u64)
    );
    assert!(matches!(records[2].event, Event::DraftPublished { .. }));
    assert_eq!(store.records(3, 1).unwrap()[0].offset, 3);
}

#[test]
fn a_changeset_published_on_two_forks_is_indexed_at_its_first_draft() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    store.create_fork(&fork("g"), &loc("l")).unwrap();
    let cs = write(&store, &main, &[("a", b"2")]);
    let first = store.publish_draft(&fork("f"), &cs).unwrap();
    let second = store.publish_draft(&fork("g"), &cs).unwrap();
    assert!(second.fresh && second.offset > first.offset);
    assert_eq!(store.replay_check().unwrap().changesets, 1);
    assert_eq!(store.changeset(first.id).unwrap(), Some(cs));
}

#[test]
fn a_changeset_that_does_not_apply_to_its_base_is_refused_and_appends_nothing() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    let len = store.len().unwrap();

    let mut forged = write(&store, &main, &[("a", b"2")]);
    forged.target = GitOid::empty_tree();
    let missing = Changeset {
        ops: vec![Op::delete(path("never-there"))],
        ..forged.clone()
    };
    let mut no_op = write(&store, &main, &[("a", b"2")]);
    no_op.ops = vec![Op::put(path("a"), Entry::file(b"1"))];
    for bad in [forged, missing, no_op] {
        assert!(
            matches!(
                store.publish_draft(&fork("f"), &bad),
                Err(CoreError::Changeset(_))
            ),
            "{bad:?}"
        );
    }
    assert_eq!(store.len().unwrap(), len);
    assert_eq!(store.fork_head(&fork("f")).unwrap(), main);
}

#[test]
fn concurrent_ingests_into_one_new_shard_all_succeed() {
    let (_d, store) = store();
    // Contents whose git ids share their first byte, so they share a shard directory.
    let mut contents: Vec<Vec<u8>> = Vec::new();
    let mut n = 0u64;
    let want = GitOid::for_blob(b"0").as_bytes()[0];
    while contents.len() < 16 {
        let c = n.to_string().into_bytes();
        if GitOid::for_blob(&c).as_bytes()[0] == want {
            contents.push(c);
        }
        n += 1;
    }
    std::thread::scope(|scope| {
        for c in &contents {
            let store = &store;
            scope.spawn(move || {
                store
                    .ingest_blob(GitOid::for_blob(c), c.len() as u64, &c[..])
                    .unwrap()
            });
        }
    });
    for c in &contents {
        assert_eq!(store.read_blob(GitOid::for_blob(c)).unwrap(), *c);
    }
}

#[test]
fn leftovers_of_an_interrupted_ingest_are_removed_on_open() {
    let dir = tempfile::tempdir().unwrap();
    drop(Store::open(dir.path()).unwrap());
    let leftover = dir.path().join("blobs/tmp/.tmpXYZ");
    std::fs::write(&leftover, b"half a blob").unwrap();
    drop(Store::open(dir.path()).unwrap());
    assert!(!leftover.exists());
}

#[test]
fn main_follows_a_fork_draft_by_draft() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"0")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l")).unwrap();
    for n in 1..=3u8 {
        let head = store.fork_head(&fork("f")).unwrap();
        let cs = write(&store, &head, &[("a", &[b'0' + n])]);
        let p = store.publish_draft(&fork("f"), &cs).unwrap();
        let at = store.advance_main(&fork("f"), p.id).unwrap();
        assert_eq!(at, p.offset + 1, "draft {n} moved main");
        assert_eq!(store.main_tree(&loc("l")).unwrap().git_tree_id(), cs.target);
    }
    assert_eq!(store.replay_check().unwrap().records, 8);
}
