//! Kill the authority during a publish, 20 times (BRO-3251 acceptance).
//!
//! `kill_during_publish` re-runs this test binary as a child that only runs
//! `child_server`: an authority on 127.0.0.1 over the state directory the parent names,
//! which writes its port to a file once it listens. The parent publishes drafts over
//! HTTP in a tight loop from a second thread, each one the next value of a counter
//! file, and SIGKILLs the child at a random moment after at least one more draft was
//! acknowledged, usually with a publish in flight. It restarts the child and sends the
//! publish it was waiting on again: the answer must be a success, `fresh` or not
//! according to whether the killed authority had recorded it, never a second record.
//! After every kill: each acknowledged draft is in the fork's history exactly once, no
//! changeset is recorded twice, the drafts are exactly the chain the counter says, and
//! the log replays cleanly.

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use cuenca_core::{Event, Store};
use cuenca_server::PrincipalKind;
use cuenca_tree::{Changeset, Tree};
use serde_json::{Value, json};

const DIR_ENV: &str = "CUENCA_SERVER_CRASH_DIR";
const NODE: &str = "nCrash11CNTRL";
const KILLS: usize = 20;

/// The child: serve until killed. Not a test on its own.
#[test]
#[ignore = "run by kill_during_publish as a child process"]
fn child_server() {
    let Ok(dir) = std::env::var(DIR_ENV) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let server = Server::start_in(&dir);
    server.whois.node(NODE);
    let port = server.url.rsplit(':').next().unwrap().to_string();
    let tmp = dir.join("port.tmp");
    std::fs::write(&tmp, port).unwrap();
    std::fs::rename(tmp, dir.join("port")).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[allow(clippy::zombie_processes)] // the caller kills and waits for it
fn spawn(dir: &Path) -> (Child, String) {
    let _ = std::fs::remove_file(dir.join("port"));
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "child_server", "--nocapture"])
        .env(DIR_ENV, dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Ok(port) = std::fs::read_to_string(dir.join("port")) {
            return (child, format!("http://127.0.0.1:{port}"));
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "child did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A client of the child authority.
struct Client {
    url: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl Client {
    fn put_blob(&self, content: &[u8]) -> Result<cuenca_tree::GitOid, String> {
        let oid = cuenca_tree::GitOid::for_blob(content);
        let r = self
            .http
            .put(format!("{}/v1/blobs/{}", self.url, oid.to_hex()))
            .bearer_auth(&self.token)
            .body(content.to_vec())
            .send()
            .map_err(|e| e.to_string())?;
        if r.status() != 200 {
            return Err(format!("blob: {}", r.status()));
        }
        Ok(oid)
    }

    fn head(&self) -> Result<Tree, String> {
        let r = self
            .http
            .get(format!("{}/v1/forks/f/tree", self.url))
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| e.to_string())?;
        let v: Value = r.json().map_err(|e| e.to_string())?;
        serde_json::from_value(v["tree"].clone()).map_err(|e| e.to_string())
    }

    /// The draft that writes counter `n` over `head`, and the content it uploaded.
    fn next(&self, head: &Tree, n: u64) -> Result<(Changeset, Vec<u8>), String> {
        let body = format!("{n}\n").repeat(256);
        let oid = self.put_blob(body.as_bytes())?;
        let cs = Changeset::new(
            head,
            vec![cuenca_tree::Op::put(
                cuenca_tree::RepoPath::new("counter").unwrap(),
                cuenca_tree::Entry::File(oid),
            )],
        )
        .unwrap();
        Ok((cs, body.into_bytes()))
    }

    fn publish(&self, cs: &Changeset) -> Result<Value, String> {
        let r = self
            .http
            .post(format!("{}/v1/forks/f/drafts", self.url))
            .bearer_auth(&self.token)
            .json(&json!({ "epoch": 1, "changeset": cs }))
            .send()
            .map_err(|e| e.to_string())?;
        let status = r.status();
        let v: Value = r.json().map_err(|e| e.to_string())?;
        if status != 200 {
            return Err(format!("publish {status}: {v}"));
        }
        Ok(v)
    }
}

#[test]
fn kill_during_publish() {
    let dir = tempfile::tempdir().unwrap();
    let (mut child, url) = spawn(dir.path());
    let admin = dir.path().join("admin.sock");
    let call = |req| cuenca_server::admin::call(&admin, &req).unwrap();
    call(cuenca_server::admin::AdminRequest::LocationAdd {
        name: cuenca_core::LocationName::new("l").unwrap(),
        remote: None,
        git: None,
    });
    let v = call(cuenca_server::admin::AdminRequest::KeyMint {
        principal: "agent".parse_name(),
        kind: PrincipalKind::Agent,
        node: cuenca_core::NodeId::new(NODE).unwrap(),
        grant: grant(&["l"], &[]),
    });
    let token = v["token"].as_str().unwrap().to_string();
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let r = http
        .post(format!("{url}/v1/forks"))
        .bearer_auth(&token)
        .json(&json!({ "fork": "f", "location": "l" }))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);

    let acked: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut url = url;
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    // How the in-flight retries were answered: (recorded before the kill, not).
    let (mut retried_recorded, mut retried_fresh) = (0, 0);
    for kill in 0..KILLS {
        let client = Arc::new(Client {
            url: url.clone(),
            token: token.clone(),
            http: http.clone(),
        });
        // The writer: publish the next counter value until a request fails (the kill).
        // It reports the draft it was waiting on when it failed.
        let acks = Arc::new(AtomicU64::new(0));
        // Raised while a publish request is out, so the kill can wait for one.
        let publishing = Arc::new(AtomicBool::new(false));
        let writer = {
            let (client, acked, acks, publishing) = (
                client.clone(),
                acked.clone(),
                acks.clone(),
                publishing.clone(),
            );
            std::thread::spawn(move || -> Option<(Changeset, Vec<u8>)> {
                loop {
                    let head = client.head().ok()?;
                    let n = counter_of(&client, &head)? + 1;
                    let (cs, content) = client.next(&head, n).ok()?;
                    publishing.store(true, Ordering::SeqCst);
                    let answer = client.publish(&cs);
                    publishing.store(false, Ordering::SeqCst);
                    match answer {
                        Ok(v) => {
                            assert_eq!(v["fresh"], true, "{v}");
                            acked.lock().unwrap().push(v["id"].as_str().unwrap().into());
                            acks.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(_) => return Some((cs, content)),
                    }
                }
            })
        };
        // Kill after 1-4 more acknowledged drafts, once a publish is out, a random
        // 0-3 ms into it (a publish takes a few ms: HTTP, then an fsynced commit).
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let target = 1 + seed % 4;
        let start = Instant::now();
        while acks.load(Ordering::SeqCst) < target || !publishing.load(Ordering::SeqCst) {
            assert!(start.elapsed() < Duration::from_secs(60), "writer stalled");
            std::hint::spin_loop();
        }
        std::thread::sleep(Duration::from_micros((seed >> 8) % 3000));
        // The sleep may have outlasted that publish: wait for one to be out again.
        while !publishing.load(Ordering::SeqCst) {
            assert!(start.elapsed() < Duration::from_secs(60), "writer stalled");
            std::hint::spin_loop();
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let in_flight = writer.join().unwrap();

        let (c, u) = spawn(dir.path());
        child = c;
        url = u;
        let client = Client {
            url: url.clone(),
            token: token.clone(),
            http: http.clone(),
        };
        // Send again the publish the kill interrupted: recorded or not, it succeeds,
        // and a recorded one is answered as the same draft, not a second. A new
        // authority does not know what the client uploaded to the old one, so a draft
        // it had not recorded is refused for its blob until the client sends the bytes
        // again, as a client does.
        if let Some((cs, content)) = in_flight {
            let answer = match client.publish(&cs) {
                Err(e) if e.contains("missing_blob") => {
                    client.put_blob(&content).unwrap();
                    client.publish(&cs)
                }
                other => other,
            };
            match answer {
                Ok(v) => {
                    if v["fresh"] == true {
                        retried_fresh += 1;
                    } else {
                        retried_recorded += 1;
                    }
                    acked.lock().unwrap().push(v["id"].as_str().unwrap().into());
                }
                // It was built on a head read before the kill, and the kill came
                // before its blob or its publish: the fork is still at that head, so
                // this cannot happen.
                Err(e) => panic!("kill {kill}: retry of the in-flight publish failed: {e}"),
            }
        }
        check(&acked.lock().unwrap(), &client, kill);
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let store = Store::open(dir.path().join("state/store")).unwrap();
    let summary = store.replay_check().unwrap();
    eprintln!(
        "{KILLS} kills; in-flight publish retried: {retried_recorded} already recorded, \
         {retried_fresh} recorded by the retry; {summary:?}"
    );
    assert!(summary.changesets >= KILLS, "{summary:?}");
    assert!(
        retried_recorded + retried_fresh >= KILLS / 2,
        "only {} of {KILLS} kills interrupted a publish",
        retried_recorded + retried_fresh
    );
}

fn counter_of(client: &Client, head: &Tree) -> Option<u64> {
    match head.get("counter") {
        None => Some(0),
        Some(e) => {
            let r = client
                .http
                .get(format!(
                    "{}/v1/forks/f/blobs/{}",
                    client.url,
                    e.oid().to_hex()
                ))
                .bearer_auth(&client.token)
                .send()
                .ok()?;
            let text = r.text().ok()?;
            text.lines().next()?.parse().ok()
        }
    }
}

/// The fork's history, read over HTTP from the restarted child.
fn check(acked: &[String], client: &Client, kill: usize) {
    let r = client
        .http
        .get(format!("{}/v1/forks/f/history", client.url))
        .bearer_auth(&client.token)
        .send()
        .unwrap();
    let records: Vec<cuenca_core::Record> = r.json().unwrap();
    let mut seen = HashSet::new();
    let mut drafts = 0u64;
    for r in &records {
        if let Event::DraftPublished { id, .. } = &r.event {
            assert!(seen.insert(id.to_hex()), "kill {kill}: {id} recorded twice");
            drafts += 1;
        }
    }
    for id in acked {
        assert!(seen.contains(id), "kill {kill}: acknowledged {id} lost");
    }
    // The drafts are the counter's chain: one per value, none missing.
    let head = client.head().unwrap();
    let n = counter_of(client, &head).unwrap();
    assert_eq!(drafts, n, "kill {kill}: {drafts} drafts for counter {n}");
}
