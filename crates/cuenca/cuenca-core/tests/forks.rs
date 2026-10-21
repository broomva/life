//! One writer per fork (spec section 2a): holders, fencing epochs, takeover, sibling
//! forks and revocation, with the spec's acceptance rows as tests.

mod common;

use std::collections::BTreeMap;
use std::sync::Barrier;

use common::*;
use cuenca_core::{CoreError, Event, ForkName, Holder, PrincipalName, Store, Writer};
use cuenca_tree::{Changeset, Entry, Op, Tree};

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn owner() -> Holder {
    holder("owner", "nOwnerMacCNTRL")
}

fn principal(s: &str) -> PrincipalName {
    PrincipalName::new(s).unwrap()
}

/// A location `l` with one file and a fork `f` held by [`agent`].
fn forked() -> (tempfile::TempDir, Store, Tree) {
    let (d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    store.create_fork(&fork("f"), &loc("l"), &agent()).unwrap();
    (d, store, main)
}

/// A small deterministic generator, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn a_new_fork_is_held_by_its_creator_under_epoch_1() {
    let (_d, store, _) = forked();
    let f = store.fork(&fork("f")).unwrap().unwrap();
    assert_eq!(
        (f.epoch(), f.holder().unwrap(), &f.parent),
        (1, &agent(), &None)
    );
    assert_eq!(f.holder_at(1), Some(&agent()));
    assert_eq!(f.holder_at(0), None);
    assert_eq!(f.holder_at(2), None);
}

/// Spec row 2 · B: a write from a principal or machine that does not hold the fork is
/// refused, 50 of 50, and appends nothing.
#[test]
fn a_write_from_anyone_but_the_holder_is_refused_50_of_50() {
    let (_d, store, main) = forked();
    // The fork has had two holders: agent (epoch 1), then owner (epoch 2).
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let len = store.len().unwrap();
    let head = store.fork(&fork("f")).unwrap().unwrap().head;
    let writers = [
        // Another principal on the holder's machine, under the current epoch.
        Writer::new(holder("intruder", "nOwnerMacCNTRL"), 2),
        // The holder's principal on another machine.
        Writer::new(holder("owner", "nOtherCNTRL"), 2),
        // Another principal on another machine.
        Writer::new(holder("intruder", "nOtherCNTRL"), 2),
        // The holder, under an epoch the fork has not reached, or 0.
        Writer::new(owner(), 3),
        Writer::new(owner(), 0),
        // A former holder under an epoch it never held.
        Writer::new(agent(), 2),
        // Someone who never held it, claiming a stale epoch: not fenced, refused.
        Writer::new(holder("intruder", "nOtherCNTRL"), 1),
    ];
    let mut refused = 0;
    for attempt in 0..50 {
        let writer = &writers[attempt % writers.len()];
        let n = rng.next().to_string();
        let cs = write(&store, &main, &[("a", n.as_bytes())]);
        let publish = store.publish_draft(&fork("f"), writer, &cs);
        let intent =
            store.record_landing_intent(&fork("f"), writer, cuenca_core::ChangesetId::of(&cs));
        match (publish, intent) {
            (Err(CoreError::NotHolder { .. }), Err(CoreError::NotHolder { .. })) => refused += 1,
            other => panic!("attempt {attempt} by {writer:?}: {other:?}"),
        }
    }
    assert_eq!(refused, 50);
    assert_eq!(store.len().unwrap(), len);
    assert_eq!(store.fork(&fork("f")).unwrap().unwrap().head, head);
    assert_eq!(store.forks().unwrap().len(), 1, "no sibling was created");
    // The holder itself still writes.
    let cs = write(&store, &main, &[("a", b"owner")]);
    let p = store
        .publish_draft(&fork("f"), &Writer::new(owner(), 2), &cs)
        .unwrap();
    assert!(p.fresh && !p.fenced && p.fork == fork("f"));
}

/// Spec row 2 · B: take a fork over while its holder is offline with unpublished
/// drafts; on reconnect each publish is rejected for the stale epoch and kept as a
/// sibling fork, with 0 files lost, 10 of 10.
#[test]
fn a_takeover_while_the_holder_is_offline_keeps_its_drafts_in_a_sibling_10_of_10() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for round in 0..10 {
        let (_d, store) = store();
        let files: Vec<(String, Vec<u8>)> = (0..1 + rng.below(8))
            .map(|i| {
                (
                    format!("d{}/f{i}", rng.below(3)),
                    rng.next().to_string().into_bytes(),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_slice()))
            .collect();
        let main = tree(&store, &borrowed);
        store.add_location(&loc("l"), None, &main).unwrap();
        store.create_fork(&fork("f"), &loc("l"), &agent()).unwrap();
        let first = write(&store, &main, &[("seen", b"by the authority")]);
        store.publish_draft(&fork("f"), &w1(), &first).unwrap();
        let published = store.fork_head(&fork("f")).unwrap();

        // Offline: the agent keeps working on its checkout, drafting changesets the
        // authority has not seen (puts, overwrites and deletes).
        let mut local = published.clone();
        let mut drafts = Vec::new();
        for d in 0..1 + rng.below(4) {
            let mut ops = Vec::new();
            for k in 0..1 + rng.below(4) {
                let content = format!("round {round} draft {d} op {k} {}", rng.next());
                let entry = Entry::File(ingest(&store, content.as_bytes()));
                ops.push(Op::put(path(&format!("new/{d}-{k}")), entry));
            }
            let existing: Vec<_> = local.entries().map(|(p, _)| p.clone()).collect();
            let victim = &existing[rng.below(existing.len() as u64) as usize];
            if rng.below(2) == 0 && !victim.as_str().starts_with("new/") {
                ops.push(Op::delete(victim.clone()));
            } else if !victim.as_str().starts_with("new/") {
                let content = format!("rewritten {}", rng.next());
                ops.push(Op::put(
                    victim.clone(),
                    Entry::File(ingest(&store, content.as_bytes())),
                ));
            }
            let cs = Changeset::new(&local, ops).unwrap();
            local = cs.apply(&local).unwrap();
            drafts.push(cs);
        }

        // The owner takes the fork over and writes to it.
        assert_eq!(store.take_over(&fork("f"), 1, &owner()).unwrap(), 2);
        let owners = write(&store, &published, &[("owner", b"was here")]);
        store
            .publish_draft(&fork("f"), &Writer::new(owner(), 2), &owners)
            .unwrap();
        let owner_head = store.fork_head(&fork("f")).unwrap();

        // The agent reconnects and publishes its drafts under epoch 1.
        let sibling = ForkName::new("f-from-nagentcntrl").unwrap();
        for (i, cs) in drafts.iter().enumerate() {
            let p = store.publish_draft(&fork("f"), &w1(), cs).unwrap();
            assert!(p.fenced && p.fresh, "round {round} draft {i}: {p:?}");
            assert_eq!(p.fork, sibling);
        }
        // A retried publish of the last draft appends nothing.
        let len = store.len().unwrap();
        let again = store
            .publish_draft(&fork("f"), &w1(), drafts.last().unwrap())
            .unwrap();
        assert!(again.fenced && !again.fresh);
        assert_eq!(store.len().unwrap(), len);

        // 0 files lost: the sibling holds exactly the agent's local tree.
        let kept = store.fork_head(&sibling).unwrap();
        let want: BTreeMap<_, _> = local.entries().map(|(p, e)| (p.clone(), *e)).collect();
        let got: BTreeMap<_, _> = kept.entries().map(|(p, e)| (p.clone(), *e)).collect();
        assert_eq!(got, want, "round {round}: files lost or changed");
        // Never merged in: the fork is the owner's.
        assert_eq!(store.fork_head(&fork("f")).unwrap(), owner_head);
        let s = store.fork(&sibling).unwrap().unwrap();
        assert_eq!(
            (
                s.parent.as_ref(),
                s.holder().unwrap(),
                s.epoch(),
                s.base,
                &s.location
            ),
            (
                Some(&fork("f")),
                &agent(),
                1,
                published.git_tree_id(),
                &loc("l")
            )
        );
        assert_eq!(store.replay_check().unwrap().forks, 2);
    }
}

#[test]
fn a_sibling_is_written_like_any_fork_by_its_holder() {
    let (_d, store, main) = forked();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let one = write(&store, &main, &[("a", b"2")]);
    let p = store.publish_draft(&fork("f"), &w1(), &one).unwrap();
    // The agent may also publish to the sibling directly, under its epoch there.
    let two = write(&store, &store.fork_head(&p.fork).unwrap(), &[("b", b"3")]);
    let q = store.publish_draft(&p.fork, &w1(), &two).unwrap();
    assert!(!q.fenced && q.fork == p.fork);
    // Out of order is refused, not reordered.
    assert!(matches!(
        store.publish_draft(&fork("f"), &w1(), &one.clone()),
        Err(CoreError::NotAtBase { .. })
    ));
    // The sibling can be taken over too; the agent's next stale publish is then refused,
    // and its checkout still holds the draft.
    store.take_over(&p.fork, 1, &owner()).unwrap();
    let three = write(&store, &store.fork_head(&p.fork).unwrap(), &[("c", b"4")]);
    let len = store.len().unwrap();
    assert!(matches!(
        store.publish_draft(&fork("f"), &w1(), &three),
        Err(CoreError::NotHolder { .. })
    ));
    assert_eq!(store.len().unwrap(), len);
}

#[test]
fn a_fork_squatting_a_siblings_name_is_never_mixed_in() {
    let (_d, store, main) = forked();
    let squat = ForkName::new("f-from-nagentcntrl").unwrap();
    store.create_fork(&squat, &loc("l"), &agent()).unwrap();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let cs = write(&store, &main, &[("a", b"2")]);
    assert!(matches!(
        store.publish_draft(&fork("f"), &w1(), &cs),
        Err(CoreError::ForkExists(name)) if name == squat.as_str()
    ));
    assert_eq!(store.fork_head(&squat).unwrap(), main);
}

#[test]
fn two_nodes_whose_ids_differ_only_in_case_do_not_share_a_sibling() {
    let (_d, store, main) = forked();
    let upper = holder("agent", "NAGENTCNTRL");
    store.take_over(&fork("f"), 1, &upper).unwrap();
    store.take_over(&fork("f"), 2, &owner()).unwrap();
    let mine = write(&store, &main, &[("a", b"mine")]);
    let p = store.publish_draft(&fork("f"), &w1(), &mine).unwrap();
    assert_eq!(p.fork.as_str(), "f-from-nagentcntrl");
    let theirs = write(&store, &main, &[("a", b"theirs")]);
    assert!(matches!(
        store.publish_draft(&fork("f"), &Writer::new(upper, 2), &theirs),
        Err(CoreError::NotHolder { .. })
    ));
    assert_eq!(store.fork_head(&p.fork).unwrap().git_tree_id(), mine.target);
}

#[test]
fn a_sibling_name_that_does_not_fit_is_refused() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    let long = ForkName::new("f".repeat(100)).unwrap();
    let node = holder("agent", &"n".repeat(30));
    store.create_fork(&long, &loc("l"), &node).unwrap();
    store.take_over(&long, 1, &owner()).unwrap();
    let cs = write(&store, &main, &[("a", b"2")]);
    let len = store.len().unwrap();
    assert!(matches!(
        store.publish_draft(&long, &Writer::new(node, 1), &cs),
        Err(CoreError::Name(_))
    ));
    assert_eq!(store.len().unwrap(), len);
}

/// Spec row 2 · B + D: revoke a principal mid-session; its next request is refused.
#[test]
fn a_revoked_principal_is_refused_on_its_next_request() {
    let (_d, store, main) = forked();
    let cs = write(&store, &main, &[("a", b"2")]);
    let p = store.publish_draft(&fork("f"), &w1(), &cs).unwrap();

    let at = store.revoke(&principal("agent")).unwrap();
    assert_eq!(store.revoked(&principal("agent")).unwrap(), Some(at));
    assert_eq!(store.revoked(&principal("owner")).unwrap(), None);
    let len = store.len().unwrap();
    let next = write(
        &store,
        &store.fork_head(&fork("f")).unwrap(),
        &[("a", b"3")],
    );
    fn revoked<T>(r: Result<T, CoreError>) -> bool {
        matches!(r, Err(CoreError::Revoked(p)) if p == "agent")
    }
    assert!(revoked(store.publish_draft(&fork("f"), &w1(), &next)));
    // Even a retry of what it published before.
    assert!(revoked(store.publish_draft(&fork("f"), &w1(), &cs)));
    assert!(revoked(store.record_landing_intent(
        &fork("f"),
        &w1(),
        p.id
    )));
    assert!(revoked(store.create_fork(&fork("g"), &loc("l"), &agent())));
    // Another principal's key on the same machine is not this one.
    store
        .create_fork(&fork("g"), &loc("l"), &holder("other", "nAgentCNTRL"))
        .unwrap();
    assert!(revoked(store.take_over(&fork("g"), 1, &agent())));
    // Revoking again is a retry.
    assert_eq!(store.revoke(&principal("agent")).unwrap(), at);
    assert_eq!(store.len().unwrap(), len + 1);

    // A revoked former holder's stale publish is refused, not kept as a sibling.
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    assert!(revoked(store.publish_draft(&fork("f"), &w1(), &next)));
    assert_eq!(store.forks().unwrap().len(), 2);
    assert_eq!(store.replay_check().unwrap().revoked, 1);
}

/// Two concurrent takeovers from the same epoch: exactly one wins.
#[test]
fn of_concurrent_takeovers_exactly_one_wins() {
    for round in 0..20 {
        let (_d, store, _) = forked();
        let contenders: Vec<Holder> = (0..8)
            .map(|i| holder(&format!("p{i}"), &format!("n{i}")))
            .collect();
        let barrier = Barrier::new(contenders.len());
        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = contenders
                .iter()
                .map(|h| {
                    let (store, barrier) = (&store, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        store.take_over(&fork("f"), 1, h)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let winners: Vec<usize> = results
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().ok().map(|_| i))
            .collect();
        assert_eq!(winners.len(), 1, "round {round}: {results:?}");
        for r in &results {
            match r {
                Ok(epoch) => assert_eq!(*epoch, 2),
                Err(CoreError::EpochMismatch {
                    expected: 1,
                    actual: 2,
                    ..
                }) => {}
                other => panic!("round {round}: {other:?}"),
            }
        }
        let f = store.fork(&fork("f")).unwrap().unwrap();
        assert_eq!(
            (f.epoch(), f.holder().unwrap()),
            (2, &contenders[winners[0]])
        );
        assert_eq!(store.len().unwrap(), 3);
    }
}

#[test]
fn a_takeover_fences_every_earlier_holder() {
    let (_d, store, main) = forked();
    let second = holder("second", "nSecondCNTRL");
    store.take_over(&fork("f"), 1, &second).unwrap();
    store.take_over(&fork("f"), 2, &owner()).unwrap();
    assert!(matches!(
        store.take_over(&fork("f"), 2, &agent()),
        Err(CoreError::EpochMismatch {
            expected: 2,
            actual: 3,
            ..
        })
    ));
    // Both earlier holders publish only to their own siblings.
    for (writer, node) in [
        (w1(), "nagentcntrl"),
        (Writer::new(second, 2), "nsecondcntrl"),
    ] {
        let cs = write(&store, &main, &[("a", node.as_bytes())]);
        let p = store.publish_draft(&fork("f"), &writer, &cs).unwrap();
        assert_eq!(p.fork.as_str(), format!("f-from-{node}"));
        // A stale holder cannot ask to land either.
        assert!(matches!(
            store.record_landing_intent(&fork("f"), &writer, p.id),
            Err(CoreError::StaleEpoch { current: 3, .. })
        ));
    }
    assert_eq!(store.fork_head(&fork("f")).unwrap(), main);
    assert!(matches!(
        store.take_over(&fork("nope"), 1, &owner()),
        Err(CoreError::UnknownFork(_))
    ));
}

#[test]
fn a_forks_history_is_the_records_that_name_it() {
    let (_d, store, main) = forked();
    store.create_fork(&fork("g"), &loc("l"), &agent()).unwrap();
    let cs = write(&store, &main, &[("a", b"2")]);
    store.publish_draft(&fork("f"), &w1(), &cs).unwrap();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let stale = write(
        &store,
        &store.fork_head(&fork("f")).unwrap(),
        &[("a", b"3")],
    );
    let p = store.publish_draft(&fork("f"), &w1(), &stale).unwrap();
    store.revoke(&principal("nobody")).unwrap();

    let kinds = |name: &ForkName| -> Vec<&'static str> {
        store
            .fork_history(name)
            .unwrap()
            .iter()
            .map(|r| match r.event {
                Event::ForkCreated { .. } => "created",
                Event::DraftPublished { .. } => "draft",
                Event::TakenOver { .. } => "taken over",
                Event::DraftFenced { .. } => "fenced",
                _ => "other",
            })
            .collect()
    };
    assert_eq!(
        kinds(&fork("f")),
        ["created", "draft", "taken over", "fenced"]
    );
    assert_eq!(kinds(&p.fork), ["fenced"]);
    assert_eq!(kinds(&fork("g")), ["created"]);
    assert_eq!(store.forks().unwrap().len(), 3);
    let summary = store.replay_check().unwrap();
    assert_eq!(
        (summary.forks, summary.revoked, summary.changesets),
        (3, 1, 2)
    );
}

#[test]
fn ownership_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).unwrap();
        let main = tree(&store, &[("a", b"1")]);
        store.add_location(&loc("l"), None, &main).unwrap();
        store.create_fork(&fork("f"), &loc("l"), &agent()).unwrap();
        store.take_over(&fork("f"), 1, &owner()).unwrap();
        store.revoke(&principal("agent")).unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    let f = store.fork(&fork("f")).unwrap().unwrap();
    assert_eq!((f.epoch(), f.holder().unwrap()), (2, &owner()));
    assert!(store.revoked(&principal("agent")).unwrap().is_some());
    assert_eq!(store.replay_check().unwrap().revoked, 1);
}
