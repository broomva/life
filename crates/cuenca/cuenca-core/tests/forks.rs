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
        let sibling = ForkName::new("f-from-nagentcntrl-e1").unwrap();
        for (i, cs) in drafts.iter().enumerate() {
            let p = store.publish_draft(&fork("f"), &w1(), cs).unwrap();
            assert!(p.fenced && p.fresh, "round {round} draft {i}: {p:?}");
            assert_eq!((&p.fork, p.epoch), (&sibling, 1));
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
    assert_eq!((p.fork.as_str(), p.epoch), ("f-from-nagentcntrl-e1", 1));
    // The agent may also publish to the sibling directly, under the epoch it was given.
    let two = write(&store, &store.fork_head(&p.fork).unwrap(), &[("b", b"3")]);
    let q = store
        .publish_draft(&p.fork, &Writer::new(agent(), p.epoch), &two)
        .unwrap();
    assert!(!q.fenced && q.fork == p.fork);
    // Out of order is refused, not reordered.
    assert!(matches!(
        store.publish_draft(&fork("f"), &w1(), &one.clone()),
        Err(CoreError::NotAtBase { .. })
    ));
}

/// A sibling taken over in turn fences the agent again: its next stale draft goes one
/// level down, never into the fork the owner now holds.
#[test]
fn a_draft_fenced_from_a_taken_over_sibling_goes_one_level_down() {
    let (_d, store, main) = forked();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let one = write(&store, &main, &[("a", b"2")]);
    let s = store.publish_draft(&fork("f"), &w1(), &one).unwrap().fork;
    store.take_over(&s, 1, &owner()).unwrap();
    let owners = write(&store, &store.fork_head(&s).unwrap(), &[("o", b"owner")]);
    store
        .publish_draft(&s, &Writer::new(owner(), 2), &owners)
        .unwrap();
    let owner_head = store.fork_head(&s).unwrap();

    let local = one.apply(&main).unwrap();
    let two = write(&store, &local, &[("c", b"4")]);
    let p = store.publish_draft(&fork("f"), &w1(), &two).unwrap();
    assert_eq!(
        (p.fork.as_str(), p.epoch, p.fenced),
        ("f-from-nagentcntrl-e1-from-nagentcntrl-e1", 1, true)
    );
    assert_eq!(store.fork_head(&p.fork).unwrap().git_tree_id(), two.target);
    assert_eq!(store.fork_head(&s).unwrap(), owner_head);
    assert_eq!(
        store.fork(&p.fork).unwrap().unwrap().parent,
        Some(s.clone())
    );
    // Published straight to the taken-over sibling, it goes the same way.
    let three = write(&store, &store.fork_head(&p.fork).unwrap(), &[("d", b"5")]);
    let q = store.publish_draft(&s, &w1(), &three).unwrap();
    assert_eq!(q.fork, p.fork);
    assert_eq!(store.replay_check().unwrap().forks, 3);
}

/// A sibling is one chain, like any fork: a second stale draft that diverges from the
/// first is refused, unacknowledged, and is kept once rebased onto the sibling's head.
#[test]
fn a_divergent_stale_draft_is_refused_until_rebased() {
    let (_d, store, main) = forked();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let x = write(&store, &main, &[("x", b"x")]);
    let y = write(&store, &main, &[("y", b"y")]);
    let p = store.publish_draft(&fork("f"), &w1(), &x).unwrap();
    let len = store.len().unwrap();
    assert!(matches!(
        store.publish_draft(&fork("f"), &w1(), &y),
        Err(CoreError::NotAtBase { .. })
    ));
    assert_eq!(store.len().unwrap(), len);
    let rebased = write(&store, &store.fork_head(&p.fork).unwrap(), &[("y", b"y")]);
    let q = store.publish_draft(&fork("f"), &w1(), &rebased).unwrap();
    assert_eq!(q.fork, p.fork);
    let head = store.fork_head(&p.fork).unwrap();
    assert!(head.get("x").is_some() && head.get("y").is_some());
}

/// A sibling starts from the head the stale epoch ended at, never from a tree the writer
/// was not given: another location's main, another fork's head, or the fork's tree from
/// before its last acknowledged draft. Each is refused as `NotAtBase`, appending nothing
/// and creating no sibling.
#[test]
fn a_sibling_starts_only_from_the_head_its_epoch_left() {
    let (_d, store, main) = forked();
    let secret = tree(&store, &[("secret.env", b"TOKEN=hunter2")]);
    store.add_location(&loc("private"), None, &secret).unwrap();
    store.create_fork(&fork("g"), &loc("l"), &owner()).unwrap();
    let theirs = write(&store, &main, &[("theirs", b"owner's work")]);
    store
        .publish_draft(&fork("g"), &Writer::new(owner(), 1), &theirs)
        .unwrap();
    let acked = write(&store, &main, &[("a", b"acked")]);
    store.publish_draft(&fork("f"), &w1(), &acked).unwrap();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let left = acked.apply(&main).unwrap();
    assert_eq!(
        store.fork(&fork("f")).unwrap().unwrap().released_at(1),
        Some(left.git_tree_id())
    );
    let len = store.len().unwrap();
    let foreign_fork = theirs.apply(&main).unwrap();
    for base in [&secret, &foreign_fork, &main] {
        let draft = write(&store, base, &[("x", b"y")]);
        assert!(
            matches!(
                store.publish_draft(&fork("f"), &w1(), &draft),
                Err(CoreError::NotAtBase { head, .. }) if head == left.git_tree_id()
            ),
            "a sibling started from {}",
            base.git_tree_id()
        );
    }
    // The acknowledged draft, retried after the takeover, is already in `f`: refused too.
    assert!(matches!(
        store.publish_draft(&fork("f"), &w1(), &acked),
        Err(CoreError::NotAtBase { .. })
    ));
    assert_eq!(store.len().unwrap(), len);
    assert_eq!(store.forks().unwrap().len(), 2);
    let next = write(&store, &left, &[("x", b"y")]);
    let p = store.publish_draft(&fork("f"), &w1(), &next).unwrap();
    assert_eq!(
        store.fork(&p.fork).unwrap().unwrap().base,
        left.git_tree_id()
    );
    store.replay_check().unwrap();
}

/// One holder fenced twice, under two epochs (a laptop that goes offline twice): each
/// episode's drafts are kept, in a sibling of their own.
#[test]
fn a_holder_fenced_under_two_epochs_keeps_both_episodes() {
    let (_d, store, main) = forked();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let d1 = write(&store, &main, &[("a", b"episode 1")]);
    let p1 = store.publish_draft(&fork("f"), &w1(), &d1).unwrap();
    // The agent gets the fork back (epoch 3), publishes, and is fenced again (epoch 4).
    store.take_over(&fork("f"), 2, &agent()).unwrap();
    let d2 = write(&store, &main, &[("b", b"accepted")]);
    store
        .publish_draft(&fork("f"), &Writer::new(agent(), 3), &d2)
        .unwrap();
    store.take_over(&fork("f"), 3, &owner()).unwrap();
    let d3 = write(&store, &d2.apply(&main).unwrap(), &[("c", b"episode 2")]);
    let p3 = store
        .publish_draft(&fork("f"), &Writer::new(agent(), 3), &d3)
        .unwrap();
    assert_eq!(p1.fork.as_str(), "f-from-nagentcntrl-e1");
    assert_eq!(p3.fork.as_str(), "f-from-nagentcntrl-e3");
    assert_eq!(store.fork_head(&p1.fork).unwrap().git_tree_id(), d1.target);
    assert_eq!(store.fork_head(&p3.fork).unwrap().git_tree_id(), d3.target);
}

/// Each stale epoch's sibling starts from the head that epoch left, not the latest one:
/// a holder fenced twice whose first episode's draft arrives after the second fencing.
#[test]
fn each_stale_epoch_resumes_from_its_own_released_head() {
    let (_d, store, main) = forked();
    let agent3 = Writer::new(agent(), 3);
    let a1 = write(&store, &main, &[("a", b"agent, epoch 1")]);
    store.publish_draft(&fork("f"), &w1(), &a1).unwrap();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    let h1 = a1.apply(&main).unwrap();
    let o2 = write(&store, &h1, &[("o", b"owner, epoch 2")]);
    store
        .publish_draft(&fork("f"), &Writer::new(owner(), 2), &o2)
        .unwrap();
    store.take_over(&fork("f"), 2, &agent()).unwrap();
    let a3 = write(&store, &o2.apply(&h1).unwrap(), &[("b", b"agent, epoch 3")]);
    store.publish_draft(&fork("f"), &agent3, &a3).unwrap();
    store.take_over(&fork("f"), 3, &owner()).unwrap();
    let h3 = a3.apply(&o2.apply(&h1).unwrap()).unwrap();
    let refused = |writer: &Writer, base: &Tree, left: &Tree| {
        let draft = write(&store, base, &[("x", b"x")]);
        assert!(matches!(
            store.publish_draft(&fork("f"), writer, &draft),
            Err(CoreError::NotAtBase { head, .. }) if head == left.git_tree_id()
        ));
    };
    refused(&w1(), &h3, &h1);
    refused(&agent3, &h1, &h3);
    let e1 = write(&store, &h1, &[("late", b"episode 1, sent late")]);
    let e3 = write(&store, &h3, &[("late", b"episode 2")]);
    let p1 = store.publish_draft(&fork("f"), &w1(), &e1).unwrap();
    let p3 = store.publish_draft(&fork("f"), &agent3, &e3).unwrap();
    assert_eq!(p1.fork.as_str(), "f-from-nagentcntrl-e1");
    assert_eq!(p3.fork.as_str(), "f-from-nagentcntrl-e3");
    assert_eq!(
        store.fork(&p1.fork).unwrap().unwrap().base,
        h1.git_tree_id()
    );
    assert_eq!(
        store.fork(&p3.fork).unwrap().unwrap().base,
        h3.git_tree_id()
    );
    store.replay_check().unwrap();
}

/// Two principals on one machine (many agents on one Mac), both fenced: each keeps its
/// drafts, in its own sibling, whichever publishes first.
#[test]
fn two_principals_on_one_node_are_each_kept() {
    let (_d, store, main) = forked();
    let agent2 = holder("agent2", "nAgentCNTRL");
    store.take_over(&fork("f"), 1, &agent2).unwrap();
    store.take_over(&fork("f"), 2, &owner()).unwrap();
    let mine = write(&store, &main, &[("a", b"agent")]);
    let theirs = write(&store, &main, &[("a", b"agent2")]);
    let p2 = store
        .publish_draft(&fork("f"), &Writer::new(agent2.clone(), 2), &theirs)
        .unwrap();
    let p1 = store.publish_draft(&fork("f"), &w1(), &mine).unwrap();
    assert_eq!(p1.fork.as_str(), "f-from-nagentcntrl-e1");
    assert_eq!(p2.fork.as_str(), "f-from-nagentcntrl-e2");
    let s2 = store.fork(&p2.fork).unwrap().unwrap();
    assert_eq!(s2.holder().unwrap(), &agent2);
    assert_eq!(
        store.fork_head(&p1.fork).unwrap().git_tree_id(),
        mine.target
    );
    assert_eq!(
        store.fork_head(&p2.fork).unwrap().git_tree_id(),
        theirs.target
    );
}

/// Node ids that differ only in case name the same text in a sibling's name, but
/// different epochs, so different siblings.
#[test]
fn two_nodes_whose_ids_differ_only_in_case_are_each_kept() {
    let (_d, store, main) = forked();
    let upper = holder("agent", "NAGENTCNTRL");
    store.take_over(&fork("f"), 1, &upper).unwrap();
    store.take_over(&fork("f"), 2, &owner()).unwrap();
    let mine = write(&store, &main, &[("a", b"mine")]);
    let theirs = write(&store, &main, &[("a", b"theirs")]);
    let p = store.publish_draft(&fork("f"), &w1(), &mine).unwrap();
    let q = store
        .publish_draft(&fork("f"), &Writer::new(upper, 2), &theirs)
        .unwrap();
    assert_ne!(p.fork, q.fork);
    assert_eq!(store.fork_head(&p.fork).unwrap().git_tree_id(), mine.target);
    assert_eq!(
        store.fork_head(&q.fork).unwrap().git_tree_id(),
        theirs.target
    );
}

/// Sibling names are kept for siblings: no one can create a fork that a stale draft
/// would later be refused by.
#[test]
fn a_fork_cannot_take_a_name_kept_for_siblings() {
    let (_d, store, _) = forked();
    for name in ["f-from-nagentcntrl-e1", "x-from.00", "a-fromage"] {
        assert!(matches!(
            store.create_fork(&fork(name), &loc("l"), &agent()),
            Err(CoreError::ReservedName(n)) if n == name
        ));
    }
    store
        .create_fork(&fork("from-here"), &loc("l"), &agent())
        .unwrap();
}

/// A fork whose name leaves no room for a readable sibling name still keeps its stale
/// drafts, under the hashed name.
#[test]
fn a_long_forks_stale_drafts_are_kept_under_a_hashed_name() {
    let (_d, store) = store();
    let main = tree(&store, &[("a", b"1")]);
    store.add_location(&loc("l"), None, &main).unwrap();
    let long = ForkName::new("f".repeat(128)).unwrap();
    let node = holder("agent", &"n".repeat(64));
    store.create_fork(&long, &loc("l"), &node).unwrap();
    store.take_over(&long, 1, &owner()).unwrap();
    let cs = write(&store, &main, &[("a", b"2")]);
    let p = store
        .publish_draft(&long, &Writer::new(node, 1), &cs)
        .unwrap();
    assert!(p.fenced && p.fork.as_str().contains("-from."), "{p:?}");
    assert_eq!(store.fork_head(&p.fork).unwrap().git_tree_id(), cs.target);
}

/// A landing intent is its holder's request: a takeover drops it.
#[test]
fn a_takeover_drops_the_old_holders_landing_intent() {
    let (_d, store, main) = forked();
    let cs = write(&store, &main, &[("a", b"2")]);
    let p = store.publish_draft(&fork("f"), &w1(), &cs).unwrap();
    store
        .record_landing_intent(&fork("f"), &w1(), p.id)
        .unwrap();
    store.take_over(&fork("f"), 1, &owner()).unwrap();
    assert_eq!(
        store.fork(&fork("f")).unwrap().unwrap().landing_intent,
        None
    );
    // The new holder asking for the same landing is a new intent, not a retry.
    let len = store.len().unwrap();
    let at = store
        .record_landing_intent(&fork("f"), &Writer::new(owner(), 2), p.id)
        .unwrap();
    assert_eq!(at, len);
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
        assert_eq!(p.fork.as_str(), format!("f-from-{node}-e{}", writer.epoch));
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
