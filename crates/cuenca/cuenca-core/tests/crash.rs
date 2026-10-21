//! Kill the writer mid-append, again and again, and check the log after each kill.
//!
//! `kill_during_append` re-runs this test binary as a child that only runs
//! `child_writer`, which in a tight loop records a landing intent for the fork's head,
//! moves main to it, and publishes the next draft, printing an ack with the draft's id
//! after each publish returns. After a restart the intent and the move are retries.
//! The parent SIGKILLs it at a random moment, reopens the store and checks: the log
//! replays cleanly (no torn, missing or duplicated record of any kind), every
//! acknowledged draft is in it, the drafts are exactly the chain the fork's head counter
//! says, and intents and moves of main trail the drafts by at most the interrupted step.

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::*;
use cuenca_core::{ChangesetId, Event, Store};
use cuenca_tree::Tree;

const DIR_ENV: &str = "CUENCA_CRASH_TEST_DIR";
const KILLS: usize = 100;
const ACKED: &str = "cuenca-crash-acked ";

fn counter(store: &Store, head: &Tree) -> u64 {
    let oid = head.get("counter").expect("counter file").oid();
    String::from_utf8(store.read_blob(oid).unwrap())
        .unwrap()
        .parse()
        .unwrap()
}

/// The child: publish drafts forever. Not a test on its own.
#[test]
#[ignore = "run by kill_during_append as a child process"]
fn child_writer() {
    let Ok(dir) = std::env::var(DIR_ENV) else {
        return;
    };
    let store = Store::open(&dir).unwrap();
    // libtest prints "test child_writer ... " without a newline first, so the parent
    // finds acks within a line rather than matching whole lines.
    loop {
        // Land the head, every time: after a restart these are retries of what the
        // killed child may already have recorded, and must append nothing.
        if let Some((head, _)) = store.fork(&fork("f")).unwrap().unwrap().head_changeset {
            store
                .record_landing_intent(&fork("f"), &w1(), head)
                .unwrap();
            store.advance_main(&fork("f"), head).unwrap();
        }
        let head = store.fork_head(&fork("f")).unwrap();
        let next = counter(&store, &head) + 1;
        let body = format!("{next}\n").repeat(512);
        let cs = write(
            &store,
            &head,
            &[
                ("counter", next.to_string().as_bytes()),
                (&format!("files/{}", next % 50), body.as_bytes()),
            ],
        );
        let published = store.publish_draft(&fork("f"), &w1(), &cs).unwrap();
        println!("{ACKED}{}", published.id);
    }
}

#[test]
fn kill_during_append() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).unwrap();
        let main = tree(&store, &[("counter", b"0")]);
        store.add_location(&loc("l"), None, &main).unwrap();
        store.create_fork(&fork("f"), &loc("l"), &agent()).unwrap();
    }
    let exe = std::env::current_exe().unwrap();
    // A small deterministic generator, so a failure reproduces.
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut acked_total = 0usize;

    for round in 0..KILLS {
        let mut child = Command::new(&exe)
            .args(["--exact", "child_writer", "--ignored", "--nocapture"])
            .arg("--test-threads=1")
            .env(DIR_ENV, dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        // Wait until the child has acknowledged its first draft of this run, so the
        // kill lands while it is appending, then let it run for 0-30 ms more.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mut early = Vec::new();
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                Ok(line) if line.contains(ACKED) => {
                    early.push(line);
                    break;
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = child.kill();
                    let out = child.wait_with_output().unwrap();
                    panic!(
                        "round {round}: no draft acknowledged within 60 s: {}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
            }
        }
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        std::thread::sleep(Duration::from_micros(seed % 30_000));
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success(), "round {round}: the child was not killed");
        reader.join().unwrap();
        let acked: Vec<ChangesetId> = early
            .into_iter()
            .chain(rx.try_iter())
            .filter_map(|l| {
                l.find(ACKED)
                    .map(|at| ChangesetId::from_hex(&l[at + ACKED.len()..]).unwrap())
            })
            .collect();
        acked_total += acked.len();

        let store = Store::open(dir.path()).unwrap();
        let summary = store
            .replay_check()
            .unwrap_or_else(|e| panic!("round {round}: replay failed: {e}"));
        for id in &acked {
            assert!(
                store.changeset(*id).unwrap().is_some(),
                "round {round}: acknowledged draft {id} is not in the log"
            );
        }
        let drafts = store
            .records(0, usize::MAX)
            .unwrap()
            .into_iter()
            .filter(|r| matches!(r.event, Event::DraftPublished { .. }))
            .count() as u64;
        let head = store.fork_head(&fork("f")).unwrap();
        assert_eq!(
            drafts,
            counter(&store, &head),
            "round {round}: drafts in the log are not the head's chain"
        );
        assert_eq!(summary.changesets as u64, drafts);
        // Each draft is followed by its intent and its move of main, so each lags the
        // one before by at most the step the kill interrupted.
        let count = |want: fn(&Event) -> bool| {
            store
                .records(0, usize::MAX)
                .unwrap()
                .iter()
                .filter(|r| want(&r.event))
                .count() as u64
        };
        let intents = count(|e| matches!(e, Event::LandingIntent { .. }));
        let moves = count(|e| matches!(e, Event::MainAdvanced { .. }));
        assert!(
            intents <= drafts && drafts <= intents + 1 && moves <= intents && intents <= moves + 1,
            "round {round}: {drafts} drafts, {intents} intents, {moves} moves of main"
        );
        assert_eq!(counter(&store, &store.main_tree(&loc("l")).unwrap()), moves);
    }
    eprintln!(
        "{KILLS} kills, each after at least one acknowledged draft; {acked_total} acknowledged in all"
    );
}
