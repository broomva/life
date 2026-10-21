//! The agent API against spec section 0: each principal's allowed and refused acts,
//! authentication on every request, revocation, takeover and the absence of owner
//! verbs.

mod common;

use std::os::unix::fs::PermissionsExt;

use common::*;
use cuenca_core::{ForkName, NodeId};
use cuenca_server::PrincipalKind::{Agent, OwnerDevice};
use cuenca_server::admin::AdminRequest;
use cuenca_tree::Tree;
use serde_json::{Value, json};

const NA: &str = "nAgentA11CNTRL";
const NB: &str = "nAgentB11CNTRL";
const ND: &str = "nDevice11CNTRL";

/// The cast: agent a1 (node A, granted l1), agent a2 (node B, granted l1 and allowed
/// to read a1's fork f1), owner device d1, and locations l1 and l2. a1 holds f1 with
/// one draft.
struct Cast {
    s: Server,
    a1: String,
    a2: String,
    d1: String,
}

impl Cast {
    fn new() -> Self {
        let s = Server::start();
        s.add_location("l1");
        s.add_location("l2");
        let a1 = s.mint("a1", Agent, NA, grant(&["l1"], &[]));
        let a2 = s.mint("a2", Agent, NB, grant(&["l1"], &["f1"]));
        let d1 = s.mint("d1", OwnerDevice, ND, grant(&[], &[]));
        s.whois.node(NA);
        let r = s.post(&a1, "/v1/forks", json!({ "fork": "f1", "location": "l1" }));
        assert_eq!(r.status(), 200, "{}", r.text().unwrap());
        let cs = s.write(&a1, &Tree::new(), &[("README", b"one\n")]);
        let r = s.publish(&a1, "f1", 1, &cs);
        assert_eq!(r.status(), 200, "{}", r.text().unwrap());
        Self { s, a1, a2, d1 }
    }

    fn names(&self, token: &str, path: &str) -> Vec<String> {
        let v: Value = self.s.get(token, path).json().unwrap();
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x["name"].as_str().unwrap().to_string())
            .collect()
    }
}

#[test]
fn an_agent_reads_its_grant_and_its_forks_and_nothing_else() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let me: Value = s.get(&c.a1, "/v1/whoami").json().unwrap();
    assert_eq!(me["principal"], "a1");
    assert_eq!(me["kind"], "agent");
    assert_eq!(c.names(&c.a1, "/v1/locations"), ["l1"]);
    assert_eq!(c.names(&c.a1, "/v1/forks"), ["f1"]);
    assert_eq!(s.get(&c.a1, "/v1/locations/l1/tree").status(), 200);
    assert_eq!(s.get(&c.a1, "/v1/locations/l2/tree").status(), 404);
    assert_eq!(s.get(&c.a1, "/v1/forks/f1/history").status(), 200);
    // l2 is not granted: no fork of it, and the refusal looks like absence.
    let r = s.post(
        &c.a1,
        "/v1/forks",
        json!({ "fork": "f2", "location": "l2" }),
    );
    assert_eq!(r.status(), 404);

    // a2 is granted l1 but did not create f1 and is not given it: invisible.
    s.whois.node(NB);
    let r = s.post(
        &c.a2,
        "/v1/forks",
        json!({ "fork": "g1", "location": "l1" }),
    );
    assert_eq!(r.status(), 200);
    assert_eq!(c.names(&c.a2, "/v1/forks"), ["f1", "g1"]);
    s.whois.node(NA);
    assert_eq!(c.names(&c.a1, "/v1/forks"), ["f1"]);
    assert_eq!(s.get(&c.a1, "/v1/forks/g1").status(), 404);
    assert_eq!(s.get(&c.a1, "/v1/forks/g1/tree").status(), 404);
    assert_eq!(s.get(&c.a1, "/v1/forks/g1/history").status(), 404);
    // Writing a fork it cannot read is refused as absence, not as "not yours".
    let cs = s.write(&c.a1, &Tree::new(), &[("x", b"x")]);
    assert_eq!(s.publish(&c.a1, "g1", 1, &cs).status(), 404);
    let body = json!({ "epoch": 1, "changeset": cuenca_core::ChangesetId::of(&cs).to_hex() });
    assert_eq!(s.post(&c.a1, "/v1/forks/g1/landing", body).status(), 404);
}

#[test]
fn an_owner_device_reads_everything_and_writes_only_its_own_forks() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(ND);
    assert_eq!(c.names(&c.d1, "/v1/locations"), ["l1", "l2"]);
    assert_eq!(c.names(&c.d1, "/v1/forks"), ["f1"]);
    let head = s.fork_head(&c.d1, "f1");
    let cs = s.write(&c.d1, &head, &[("README", b"device\n")]);
    let r = s.publish(&c.d1, "f1", 1, &cs);
    assert_eq!(r.status(), 403);
    assert_eq!(r.json::<Value>().unwrap()["error"], "not_holder");
    let r = s.post(&c.d1, "/v1/forks", json!({ "fork": "d", "location": "l2" }));
    assert_eq!(r.status(), 200);
    let cs = s.write(&c.d1, &Tree::new(), &[("note", b"mine\n")]);
    assert_eq!(s.publish(&c.d1, "d", 1, &cs).status(), 200);
}

/// Spec row: a publish to a fork from a principal or machine that does not hold it is
/// refused, 100% of 50 attempts.
#[test]
fn fifty_publishes_by_non_holders_are_all_refused() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let head = s.fork_head(&c.a1, "f1");
    let cs = s.write(&c.a1, &head, &[("README", b"two\n")]);
    // Every writer has the bytes, so the refusal is about holding the fork.
    s.whois.node(NB);
    s.put_blob(&c.a2, b"two\n");
    s.whois.node(ND);
    s.put_blob(&c.d1, b"two\n");
    let len = s.authority.store().len().unwrap();
    for i in 0..50u64 {
        let epoch = 1 + i % 3;
        let r = match i % 5 {
            // a2 reads f1 but does not hold it.
            0 | 1 => {
                s.whois.node(NB);
                s.publish(&c.a2, "f1", epoch, &cs)
            }
            // an owner device reads every fork and holds none of a1's.
            2 => {
                s.whois.node(ND);
                s.publish(&c.d1, "f1", epoch, &cs)
            }
            // a1's own key from a2's machine: not a1's node.
            3 => {
                s.whois.node(NB);
                s.publish(&c.a1, "f1", epoch, &cs)
            }
            // a1's key on its node, but a holder that never was: wrong epoch.
            _ => {
                s.whois.node(NA);
                s.publish(&c.a1, "f1", epoch + 1, &cs)
            }
        };
        assert!(
            [401, 403].contains(&r.status().as_u16()),
            "attempt {i}: {} {}",
            r.status(),
            r.text().unwrap()
        );
    }
    assert_eq!(s.authority.store().len().unwrap(), len, "nothing appended");
    s.whois.node(NA);
    assert_eq!(s.publish(&c.a1, "f1", 1, &cs).status(), 200);
}

#[test]
fn every_request_is_authenticated_ten_of_ten_each() {
    let c = Cast::new();
    let s = &c.s;
    let id = c.a1.split('_').nth(1).unwrap();
    let mut flipped = c.a1.clone().into_bytes();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == b'0' { b'1' } else { b'0' };
    let flipped = String::from_utf8(flipped).unwrap();
    type Case<'a> = (&'a str, Box<dyn Fn() + 'a>, String);
    let cases: Vec<Case> = vec![
        ("no key", Box::new(|| s.whois.node(NA)), String::new()),
        ("garbage", Box::new(|| s.whois.node(NA)), "garbage".into()),
        (
            "wrong secret",
            Box::new(|| s.whois.node(NA)),
            flipped.clone(),
        ),
        (
            "id only",
            Box::new(|| s.whois.node(NA)),
            format!("cuenca_{id}_"),
        ),
        ("other node", Box::new(|| s.whois.node(NB)), c.a1.clone()),
        (
            "unnamed source",
            Box::new(|| s.whois.set(Ok(None))),
            c.a1.clone(),
        ),
        (
            "whois failed",
            Box::new(|| s.whois.set(Err("down".into()))),
            c.a1.clone(),
        ),
        (
            "node serves a route",
            Box::new(|| s.whois.set(Ok(Some(peer(NA, true))))),
            c.a1.clone(),
        ),
    ];
    for (what, setup, token) in &cases {
        setup();
        for _ in 0..10 {
            for path in ["/v1/whoami", "/v1/forks", "/v1/forks/f1/tree"] {
                let mut req = s.client.get(format!("{}{path}", s.url));
                if !token.is_empty() {
                    req = req.bearer_auth(token);
                }
                let r = req.send().unwrap();
                assert_eq!(r.status(), 401, "{what} {path}");
                let body = r.text().unwrap();
                assert!(
                    !body.contains("node"),
                    "{what}: the answer says nothing: {body}"
                );
            }
        }
    }
    s.whois.node(NA);
    assert_eq!(s.get(&c.a1, "/v1/whoami").status(), 200);
}

#[test]
fn a_revoked_key_is_refused_on_its_next_request_and_its_name_is_not_reused() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    assert_eq!(s.get(&c.a1, "/v1/whoami").status(), 200);
    let v = s.admin(AdminRequest::KeyRevoke {
        principal: "a1".parse_name(),
    });
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["had_key"], true);
    assert_eq!(s.get(&c.a1, "/v1/whoami").status(), 401);
    let head = Tree::new();
    s.whois.node(ND);
    let cs = s.write(&c.d1, &head, &[("x", b"x")]);
    s.whois.node(NA);
    assert_eq!(s.publish(&c.a1, "f1", 1, &cs).status(), 401);
    let v = s.admin(AdminRequest::KeyMint {
        principal: "a1".parse_name(),
        kind: Agent,
        node: NodeId::new(NA).unwrap(),
        grant: grant(&["l1"], &[]),
    });
    assert_eq!(v["ok"], false, "{v}");
    // The log refuses the principal even with a key file that still lists it.
    let still = s.mint("a3", Agent, NA, grant(&["l1"], &[]));
    s.authority.store().revoke(&"a3".parse_name()).unwrap();
    assert_eq!(s.get(&still, "/v1/whoami").status(), 401);
}

#[test]
fn after_a_takeover_the_old_holder_publishes_to_a_sibling() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let head = s.fork_head(&c.a1, "f1");
    let v = s.admin(AdminRequest::TakeOver {
        fork: ForkName::new("f1").unwrap(),
        expected_epoch: 1,
        principal: "d1".parse_name(),
        node: NodeId::new(ND).unwrap(),
    });
    assert_eq!(v["epoch"], 2, "{v}");
    // The same takeover again is a compare-and-swap that fails.
    let v = s.admin(AdminRequest::TakeOver {
        fork: ForkName::new("f1").unwrap(),
        expected_epoch: 1,
        principal: "d1".parse_name(),
        node: NodeId::new(ND).unwrap(),
    });
    assert_eq!(v["ok"], false, "{v}");
    let cs = s.write(&c.a1, &head, &[("offline", b"kept\n")]);
    let r: Value = s.publish(&c.a1, "f1", 1, &cs).json().unwrap();
    assert_eq!(r["fenced"], true, "{r}");
    let sibling = r["fork"].as_str().unwrap();
    assert!(sibling.starts_with("f1-from-"), "{sibling}");
    // The former holder reads its sibling, and no longer reads the fork itself.
    assert_eq!(c.names(&c.a1, "/v1/forks"), [sibling.to_string()]);
    assert_eq!(s.get(&c.a1, "/v1/forks/f1").status(), 404);
    assert_eq!(s.get(&c.a1, "/v1/forks/f1/tree").status(), 404);
    let tree = s.fork_head(&c.a1, sibling);
    assert!(tree.get("offline").is_some());
    // The new holder writes f1 under epoch 2. It may move a file a1 wrote, which it
    // never uploaded: the blob is in the fork's head, which it reads.
    s.whois.node(ND);
    let readme = head.get("README").unwrap().oid();
    let moved = cuenca_tree::Changeset::new(
        &head,
        vec![
            cuenca_tree::Op::delete(cuenca_tree::RepoPath::new("README").unwrap()),
            cuenca_tree::Op::put(
                cuenca_tree::RepoPath::new("README.md").unwrap(),
                cuenca_tree::Entry::File(readme),
            ),
        ],
    )
    .unwrap();
    let r = s.publish(&c.d1, "f1", 2, &moved);
    assert_eq!(r.status(), 200, "{}", r.text().unwrap());
    let head = s.fork_head(&c.d1, "f1");
    let cs = s.write(&c.d1, &head, &[("README", b"device\n")]);
    assert_eq!(s.publish(&c.d1, "f1", 2, &cs).status(), 200);
}

#[test]
fn publishes_and_landing_intents_are_idempotent() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let head = s.fork_head(&c.a1, "f1");
    let cs = s.write(&c.a1, &head, &[("README", b"two\n")]);
    let first: Value = s.publish(&c.a1, "f1", 1, &cs).json().unwrap();
    assert_eq!(first["fresh"], true);
    let again: Value = s.publish(&c.a1, "f1", 1, &cs).json().unwrap();
    assert_eq!(again["fresh"], false);
    assert_eq!(again["offset"], first["offset"]);
    let id = first["id"].as_str().unwrap();
    let body = json!({ "epoch": 1, "changeset": id });
    let one: Value = s
        .post(&c.a1, "/v1/forks/f1/landing", body.clone())
        .json()
        .unwrap();
    let two: Value = s
        .post(&c.a1, "/v1/forks/f1/landing", body.clone())
        .json()
        .unwrap();
    assert_eq!(one["offset"], two["offset"]);
    // A reader that does not hold the fork cannot ask to land it.
    s.whois.node(NB);
    assert_eq!(s.post(&c.a2, "/v1/forks/f1/landing", body).status(), 403);
    // A stale draft is refused with the head it should rebase on.
    s.whois.node(NA);
    let stale = s.write(&c.a1, &head, &[("other", b"x")]);
    let r = s.publish(&c.a1, "f1", 1, &stale);
    assert_eq!(r.status(), 409);
    let r: Value = r.json().unwrap();
    assert_eq!(r["error"], "not_at_base");
    assert_eq!(r["head"], cs.target.to_hex());
}

#[test]
fn blobs_are_verified_on_upload_and_read_only_through_a_visible_tree() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let head = s.fork_head(&c.a1, "f1");
    let oid = head.get("README").unwrap().oid();
    let r = s.get(&c.a1, &format!("/v1/forks/f1/blobs/{}", oid.to_hex()));
    assert_eq!(r.status(), 200);
    assert_eq!(r.bytes().unwrap().as_ref(), b"one\n");
    // Not in the tree asked through: 404 even though the store has it.
    let other = s.put_blob(&c.a1, b"loose\n");
    let r = s.get(&c.a1, &format!("/v1/forks/f1/blobs/{}", other.to_hex()));
    assert_eq!(r.status(), 404);
    // Not a fork it reads.
    s.whois.node(NB);
    let r = s.get(&c.a2, &format!("/v1/locations/l2/blobs/{}", oid.to_hex()));
    assert_eq!(r.status(), 404);
    // Wrong content for the claimed id, whether or not the store holds that id.
    for claimed in [oid, cuenca_tree::GitOid::for_blob(b"never uploaded")] {
        let r = s
            .client
            .put(format!("{}/v1/blobs/{}", s.url, claimed.to_hex()))
            .bearer_auth(&c.a2)
            .body(b"not it".to_vec())
            .send()
            .unwrap();
        assert_eq!(r.status(), 422, "{claimed}");
    }
    // A draft naming a blob nobody uploaded is refused.
    s.whois.node(NA);
    let missing = cuenca_tree::GitOid::for_blob(b"absent");
    let cs = cuenca_tree::Changeset::new(
        &head,
        vec![cuenca_tree::Op::put(
            cuenca_tree::RepoPath::new("absent").unwrap(),
            cuenca_tree::Entry::File(missing),
        )],
    )
    .unwrap();
    assert_eq!(s.publish(&c.a1, "f1", 1, &cs).status(), 422);
}

/// Spec row: an agent tries to mint a key, register a location, take over a fork or
/// land a location without a remote through the HTTP API. None has a route.
#[test]
fn no_owner_act_has_a_route() {
    let c = Cast::new();
    let s = &c.s;
    let attempts: [(&str, &str); 9] = [
        ("POST", "/v1/keys"),
        ("POST", "/v1/keys/mint"),
        ("DELETE", "/v1/keys/a1"),
        ("POST", "/v1/locations"),
        ("PUT", "/v1/locations/l3"),
        ("POST", "/v1/forks/f1/take-over"),
        ("POST", "/v1/forks/f1/takeover"),
        ("POST", "/v1/locations/l1/land"),
        ("POST", "/v1/admin"),
    ];
    for (token, node) in [(&c.a1, NA), (&c.d1, ND)] {
        s.whois.node(node);
        let len = s.authority.store().len().unwrap();
        for (method, path) in attempts {
            let r = s
                .client
                .request(method.parse().unwrap(), format!("{}{path}", s.url))
                .bearer_auth(token)
                .json(&json!({ "principal": "x", "name": "l3", "expected_epoch": 1 }))
                .send()
                .unwrap();
            assert!(
                [404, 405].contains(&r.status().as_u16()),
                "{method} {path}: {}",
                r.status()
            );
        }
        assert_eq!(s.authority.store().len().unwrap(), len);
    }
}

#[test]
fn the_admin_socket_is_private_and_lists_no_secrets() {
    let c = Cast::new();
    let mode = std::fs::metadata(&c.s.admin).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let v = c.s.admin(AdminRequest::KeyList);
    let listed = v.to_string();
    for token in [&c.a1, &c.a2, &c.d1] {
        let secret = token.rsplit('_').next().unwrap();
        assert!(!listed.contains(secret));
    }
    assert_eq!(v["keys"].as_array().unwrap().len(), 3);
    let raw = std::os::unix::net::UnixStream::connect(&c.s.admin).unwrap();
    use std::io::{BufRead, Write};
    (&raw).write_all(b"{\"op\":\"nope\"}\n").unwrap();
    let mut line = String::new();
    std::io::BufReader::new(&raw).read_line(&mut line).unwrap();
    assert!(line.contains("\"ok\":false"), "{line}");
}

#[test]
fn a_location_registers_from_a_git_repository() {
    let s = Server::start();
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-q"]);
    std::fs::write(repo.path().join("a.txt"), "hello\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "one"]);
    let tree = git(&["rev-parse", "HEAD^{tree}"]);
    let v = s.admin(AdminRequest::LocationAdd {
        name: cuenca_core::LocationName::new("repo").unwrap(),
        remote: Some("https://github.com/broomva/example.git".into()),
        git: Some(cuenca_server::admin::GitSource {
            repo: repo.path().to_path_buf(),
            rev: "HEAD".into(),
        }),
    });
    assert_eq!(v["main"], tree.trim(), "{v}");
    let d = s.mint("d1", OwnerDevice, ND, grant(&[], &[]));
    s.whois.node(ND);
    let r: Value = s.get(&d, "/v1/locations/repo/tree").json().unwrap();
    assert_eq!(r["oid"], tree.trim());
    let blob = cuenca_tree::GitOid::for_blob(b"hello\n");
    let r = s.get(&d, &format!("/v1/locations/repo/blobs/{}", blob.to_hex()));
    assert_eq!(r.bytes().unwrap().as_ref(), b"hello\n");
}

#[test]
fn the_authority_refuses_a_non_tailnet_bind() {
    let dir = tempfile::tempdir().unwrap();
    for bind in ["0.0.0.0:0", "127.0.0.1:0", "[::]:0"] {
        let config = cuenca_server::Config {
            state: dir.path().join("state"),
            bind: bind.parse().unwrap(),
            admin_socket: dir.path().join("admin.sock"),
            unsafe_bind: false,
            tailscale: "tailscale".into(),
            min_free: cuenca_server::DEFAULT_MIN_FREE,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(cuenca_server::run(config)).unwrap_err();
        assert!(err.to_string().contains("not a tailnet address"), "{err}");
    }
    assert!(
        !dir.path().join("state").exists(),
        "refused before touching state"
    );
}

/// A draft may not name a blob the caller can neither read nor has uploaded: the store
/// holds it, but naming it would let the caller read it back through its own fork. The
/// refusal is the one an absent blob gets.
#[test]
fn a_draft_cannot_launder_a_blob_the_caller_cannot_read() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let head = s.fork_head(&c.a1, "f1");
    let secret = s.write(&c.a1, &head, &[("secret.txt", b"TOP SECRET of a1\n")]);
    assert_eq!(s.publish(&c.a1, "f1", 1, &secret).status(), 200);
    let held = s.fork_head(&c.a1, "f1").get("secret.txt").unwrap().oid();
    let absent = cuenca_tree::GitOid::for_blob(b"nobody has this");
    s.whois.node(NB);
    let r = s.post(&c.a2, "/v1/forks", json!({ "fork": "g", "location": "l1" }));
    assert_eq!(r.status(), 200);
    let put = |oid| {
        cuenca_tree::Changeset::new(
            &s.fork_head(&c.a2, "g"),
            vec![cuenca_tree::Op::put(
                cuenca_tree::RepoPath::new("x").unwrap(),
                cuenca_tree::Entry::File(oid),
            )],
        )
        .unwrap()
    };
    let answers: Vec<(u16, Value)> = [held, absent]
        .into_iter()
        .map(|oid| {
            let r = s.publish(&c.a2, "g", 1, &put(oid));
            (r.status().as_u16(), r.json().unwrap())
        })
        .collect();
    for (status, body) in &answers {
        assert_eq!(*status, 422, "{body}");
        assert_eq!(body["error"], "missing_blob");
    }
    let r = s.get(&c.a2, &format!("/v1/forks/g/blobs/{}", held.to_hex()));
    assert_eq!(r.status(), 404);
    // Sending the bytes proves a2 has them, and then it may name them.
    s.put_blob(&c.a2, b"TOP SECRET of a1\n");
    assert_eq!(s.publish(&c.a2, "g", 1, &put(held)).status(), 200);
    // A blob already in the fork's own trees needs no upload: a1 drafts on top of its head.
    s.whois.node(NA);
    let head = s.fork_head(&c.a1, "f1");
    let renamed = cuenca_tree::Changeset::new(
        &head,
        vec![
            cuenca_tree::Op::delete(cuenca_tree::RepoPath::new("secret.txt").unwrap()),
            cuenca_tree::Op::put(
                cuenca_tree::RepoPath::new("moved.txt").unwrap(),
                cuenca_tree::Entry::File(held),
            ),
        ],
    )
    .unwrap();
    assert_eq!(s.publish(&c.a1, "f1", 1, &renamed).status(), 200);
}

/// What a principal uploaded counts for that principal only (and only until the
/// authority restarts: a client sends the bytes again then).
#[test]
fn possession_is_per_principal() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NB);
    let r = s.post(&c.a2, "/v1/forks", json!({ "fork": "g", "location": "l1" }));
    assert_eq!(r.status(), 200);
    s.whois.node(NA);
    let oid = s.put_blob(&c.a1, b"a1 sent this\n");
    s.whois.node(NB);
    let cs = cuenca_tree::Changeset::new(
        &Tree::new(),
        vec![cuenca_tree::Op::put(
            cuenca_tree::RepoPath::new("y").unwrap(),
            cuenca_tree::Entry::File(oid),
        )],
    )
    .unwrap();
    assert_eq!(s.publish(&c.a2, "g", 1, &cs).status(), 422);
}

/// Spec section 2a: an agent reads the forks its grant names. A draft fenced into a
/// sibling is the sibling's content, so a reader of the fork who does not read the
/// sibling does not see it in the fork's history.
#[test]
fn history_hides_drafts_fenced_into_a_sibling_the_reader_cannot_read() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let head = s.fork_head(&c.a1, "f1");
    let v = s.admin(AdminRequest::TakeOver {
        fork: ForkName::new("f1").unwrap(),
        expected_epoch: 1,
        principal: "d1".parse_name(),
        node: NodeId::new(ND).unwrap(),
    });
    assert_eq!(v["epoch"], 2, "{v}");
    let cs = s.write(&c.a1, &head, &[("a-private.txt", b"stale writer's\n")]);
    let r: Value = s.publish(&c.a1, "f1", 1, &cs).json().unwrap();
    let sibling = r["fork"].as_str().unwrap().to_string();
    let events = |token: &str, fork: &str| -> Vec<String> {
        let v: Value = s
            .get(token, &format!("/v1/forks/{fork}/history"))
            .json()
            .unwrap();
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| r["event"]["event"].as_str().unwrap().to_string())
            .collect()
    };
    // a2 reads f1 by grant, not the sibling.
    s.whois.node(NB);
    assert_eq!(s.get(&c.a2, &format!("/v1/forks/{sibling}")).status(), 404);
    let seen = events(&c.a2, "f1");
    assert!(!seen.contains(&"draft_fenced".to_string()), "{seen:?}");
    assert!(seen.contains(&"taken_over".to_string()), "{seen:?}");
    let raw = s.get(&c.a2, "/v1/forks/f1/history").text().unwrap();
    assert!(!raw.contains("a-private.txt"), "{raw}");
    // The owner device reads both, and sees it.
    s.whois.node(ND);
    assert!(events(&c.d1, "f1").contains(&"draft_fenced".to_string()));
    // So does the stale writer, through its sibling.
    s.whois.node(NA);
    assert!(events(&c.a1, &sibling).contains(&"draft_fenced".to_string()));
}

/// No request body is read before the caller is authenticated.
#[test]
fn bodies_are_not_read_before_authentication() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    for path in ["/v1/forks", "/v1/forks/f1/drafts", "/v1/forks/f1/landing"] {
        for token in ["", "garbage"] {
            let mut req = s
                .client
                .post(format!("{}{path}", s.url))
                .header("content-type", "application/json")
                .body("{ not json");
            if !token.is_empty() {
                req = req.bearer_auth(token);
            }
            assert_eq!(req.send().unwrap().status(), 401, "{path} {token:?}");
        }
    }
    let r = s
        .client
        .put(format!("{}/v1/blobs/{}", s.url, "0".repeat(40)))
        .body(vec![0u8; 1024])
        .send()
        .unwrap();
    assert_eq!(r.status(), 401);
}

/// A chunked upload longer than the Content-Length it states is refused, and what was
/// staged is gone.
#[test]
fn an_upload_longer_than_it_says_is_refused() {
    use std::io::{Read, Write};
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let oid = cuenca_tree::GitOid::for_blob(b"abc");
    let addr = s.url.trim_start_matches("http://");
    let mut tcp = std::net::TcpStream::connect(addr).unwrap();
    let chunk = vec![b'z'; 4096];
    let mut req = format!(
        "PUT /v1/blobs/{} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\n\
         Content-Length: 3\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        oid.to_hex(),
        c.a1
    )
    .into_bytes();
    for _ in 0..4 {
        req.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        req.extend_from_slice(&chunk);
        req.extend_from_slice(b"\r\n");
    }
    req.extend_from_slice(b"0\r\n\r\n");
    tcp.write_all(&req).unwrap();
    let mut answer = String::new();
    let _ = tcp.read_to_string(&mut answer);
    let status = answer.split(' ').nth(1).unwrap_or("");
    // hyper drops a Content-Length sent with Transfer-Encoding (RFC 9112 6.3), so this
    // is refused as unsized; a length-checked body never runs past what it said.
    assert!(["400", "411", "422"].contains(&status), "{answer}");
    std::thread::sleep(std::time::Duration::from_millis(100));
    let staging = s.authority.staging_dir();
    assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
}

#[test]
fn staging_left_by_a_crash_is_cleared_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(state.join("staging")).unwrap();
    std::fs::write(state.join("staging/.tmpXYZ"), b"half an upload").unwrap();
    let whois: std::sync::Arc<dyn cuenca_server::Whois> = std::sync::Arc::new(Never);
    let a = cuenca_server::Authority::open(&state, whois).unwrap();
    assert_eq!(std::fs::read_dir(a.staging_dir()).unwrap().count(), 0);
}

struct Never;

impl cuenca_server::Whois for Never {
    fn whois(&self, _ip: std::net::IpAddr) -> cuenca_server::WhoisFuture<'_> {
        Box::pin(async { Ok(None) })
    }
}

#[test]
fn the_admin_socket_refuses_a_directory_others_can_write() {
    let dir = tempfile::tempdir().unwrap();
    let open = dir.path().join("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let err = cuenca_server::admin::bind(&open.join("admin.sock")).unwrap_err();
    assert!(err.to_string().contains("owner only"), "{err}");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(cuenca_server::admin::bind(&open.join("admin.sock")).is_ok());
}

/// Uploads leave the free-space reserve: past it they are refused, and the log keeps
/// room for its own writes.
#[test]
fn uploads_keep_the_free_space_reserve() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    s.authority.set_min_free(u64::MAX / 2);
    let content = b"no room for me\n";
    let oid = cuenca_tree::GitOid::for_blob(content);
    let r = s
        .client
        .put(format!("{}/v1/blobs/{}", s.url, oid.to_hex()))
        .bearer_auth(&c.a1)
        .body(content.to_vec())
        .send()
        .unwrap();
    assert_eq!(r.status(), 507);
    assert!(s.authority.store().blob_info(oid).unwrap().is_none());
    s.authority.set_min_free(0);
    assert_eq!(s.put_blob(&c.a1, content), oid);
}

/// After a takeover the former holder reads the fork no more, and its stale drafts,
/// which land in a sibling it reads, may not name the new holder's blobs: not as a
/// read-back, and not as an oracle (a hit and a miss get the same answer, before any
/// base check).
#[test]
fn a_former_holder_cannot_launder_or_probe_the_new_holders_blobs() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let released = s.fork_head(&c.a1, "f1");
    let v = s.admin(AdminRequest::TakeOver {
        fork: ForkName::new("f1").unwrap(),
        expected_epoch: 1,
        principal: "a2".parse_name(),
        node: NodeId::new(NB).unwrap(),
    });
    assert_eq!(v["epoch"], 2, "{v}");
    s.whois.node(NB);
    let head = s.fork_head(&c.a2, "f1");
    let cs = s.write(&c.a2, &head, &[("pin.txt", b"PIN=4821\n")]);
    assert_eq!(s.publish(&c.a2, "f1", 2, &cs).status(), 200);
    let secret = cuenca_tree::GitOid::for_blob(b"PIN=4821\n");
    let miss = cuenca_tree::GitOid::for_blob(b"PIN=4820\n");
    s.whois.node(NA);
    let stale = |base: &Tree, oid| {
        cuenca_tree::Changeset::new(
            base,
            vec![cuenca_tree::Op::put(
                cuenca_tree::RepoPath::new("got.txt").unwrap(),
                cuenca_tree::Entry::File(oid),
            )],
        )
        .unwrap()
    };
    for base in [&released, &Tree::new()] {
        let answers: Vec<(u16, String)> = [secret, miss]
            .into_iter()
            .map(|oid| {
                let r = s.publish(&c.a1, "f1", 1, &stale(base, oid));
                let status = r.status().as_u16();
                let v: Value = r.json().unwrap();
                (status, v["error"].as_str().unwrap_or("").to_string())
            })
            .collect();
        assert_eq!(answers[0], answers[1], "a hit and a miss answer alike");
        assert_eq!(answers[0], (422, "missing_blob".to_string()));
    }
    // With its own bytes, the stale draft lands in a1's sibling as before.
    let mine = s.write(&c.a1, &released, &[("offline.txt", b"a1 wrote this\n")]);
    let r: Value = s.publish(&c.a1, "f1", 1, &mine).json().unwrap();
    assert_eq!(r["fenced"], true, "{r}");
}

/// Uploads in flight hold their room: a second upload cannot pass on space the first
/// has been promised, and the room comes back when the first ends.
#[test]
fn uploads_in_flight_hold_their_room() {
    use std::io::Write;
    const MIB: u64 = 1024 * 1024;
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let free = s.authority.free_bytes().unwrap();
    assert!(free > 1024 * MIB, "this test needs 1 GiB free, has {free}");
    // 300 MiB above the reserve: one 100 MiB upload (200 MiB held) fits, two do not.
    s.authority.set_min_free(free - 300 * MIB);
    let addr = s.url.trim_start_matches("http://").to_string();
    let open_upload = |size: u64| {
        let mut tcp = std::net::TcpStream::connect(&addr).unwrap();
        let head = format!(
            "PUT /v1/blobs/{} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\n\
             Content-Length: {size}\r\n\r\nx",
            "1".repeat(40),
            c.a1
        );
        tcp.write_all(head.as_bytes()).unwrap();
        tcp
    };
    let first = open_upload(100 * MIB);
    std::thread::sleep(std::time::Duration::from_millis(300));
    // The first bytes of the answer, or "" if none came within 2 s (the server is
    // waiting for the body: the upload passed the room check).
    let probe = |size: u64| {
        let mut tcp = open_upload(size);
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut answer = [0u8; 12];
        use std::io::Read;
        match tcp.read(&mut answer) {
            Ok(n) => String::from_utf8_lossy(&answer[..n]).to_string(),
            Err(e) => {
                let quiet = [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut];
                assert!(quiet.contains(&e.kind()), "{e}");
                String::new()
            }
        }
    };
    assert!(
        probe(100 * MIB).contains("507"),
        "the first upload's room is held"
    );
    drop(first);
    std::thread::sleep(std::time::Duration::from_millis(300));
    // Its room is back: the same upload now passes the check and waits for its body.
    assert_eq!(
        probe(100 * MIB),
        "",
        "the room came back: the upload waits for its body"
    );
}

/// Revocation's first step needs no write: a banned principal is refused at once, with
/// nothing yet in the log (as when the disk is full).
#[test]
fn a_ban_refuses_at_once_without_a_write() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    assert_eq!(s.get(&c.a1, "/v1/whoami").status(), 200);
    let len = s.authority.store().len().unwrap();
    s.authority.ban(&"a1".parse_name());
    assert_eq!(s.get(&c.a1, "/v1/whoami").status(), 401);
    assert_eq!(s.authority.store().len().unwrap(), len);
    assert!(
        s.authority
            .store()
            .revoked(&"a1".parse_name())
            .unwrap()
            .is_none()
    );
}

/// Every agent write keeps the free-space reserve, not only uploads.
#[test]
fn every_write_keeps_the_free_space_reserve() {
    let c = Cast::new();
    let s = &c.s;
    s.whois.node(NA);
    let head = s.fork_head(&c.a1, "f1");
    let cs = s.write(&c.a1, &head, &[("README", b"two\n")]);
    let len = s.authority.store().len().unwrap();
    s.authority.set_min_free(u64::MAX / 2);
    assert_eq!(s.publish(&c.a1, "f1", 1, &cs).status(), 507);
    let r = s.post(
        &c.a1,
        "/v1/forks",
        json!({ "fork": "f9", "location": "l1" }),
    );
    assert_eq!(r.status(), 507);
    let id = cuenca_core::ChangesetId::of(&cs).to_hex();
    let r = s.post(
        &c.a1,
        "/v1/forks/f1/landing",
        json!({ "epoch": 1, "changeset": id }),
    );
    assert_eq!(r.status(), 507);
    assert_eq!(s.authority.store().len().unwrap(), len, "nothing appended");
    s.authority.set_min_free(0);
    assert_eq!(s.publish(&c.a1, "f1", 1, &cs).status(), 200);
}

/// `--min-free` reaches the authority.
#[test]
fn the_configured_reserve_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let config = cuenca_server::Config {
        state: dir.path().join("state"),
        bind: "100.64.0.1:1".parse().unwrap(),
        admin_socket: dir.path().join("admin.sock"),
        unsafe_bind: false,
        tailscale: "tailscale".into(),
        min_free: 12_345,
    };
    let whois: std::sync::Arc<dyn cuenca_server::Whois> = std::sync::Arc::new(Never);
    let a = cuenca_server::open_authority(&config, whois).unwrap();
    assert_eq!(a.min_free(), 12_345);
}

fn send_partial(s: &Server, request_head: String) -> std::net::TcpStream {
    use std::io::Write;
    let addr = s.url.trim_start_matches("http://");
    let mut tcp = std::net::TcpStream::connect(addr).unwrap();
    tcp.write_all(request_head.as_bytes()).unwrap();
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    tcp
}

fn status_of(tcp: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    let mut answer = [0u8; 12];
    let n = tcp.read(&mut answer).unwrap_or(0);
    String::from_utf8_lossy(&answer[..n]).to_string()
}

fn stalled_upload(s: &Server, token: &str) -> std::net::TcpStream {
    send_partial(
        s,
        format!(
            "PUT /v1/blobs/{} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\
             Content-Length: 10\r\n\r\nx",
            "1".repeat(40)
        ),
    )
}

/// A stalled upload body is dropped after the idle limit, and its room is given back.
#[test]
fn a_stalled_upload_is_dropped_and_its_room_returned() {
    let s = Server::start_with(cuenca_server::Limits {
        body_idle: std::time::Duration::from_millis(300),
        ..Default::default()
    });
    let a1 = s.mint("a1", Agent, NA, grant(&[], &[]));
    s.whois.node(NA);
    let mut tcp = stalled_upload(&s, &a1);
    let started = std::time::Instant::now();
    assert!(status_of(&mut tcp).contains("408"));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    // Nothing is held: a 1 MiB upload (2 MiB of room) fits in 3 MiB above the reserve.
    s.authority
        .set_min_free(s.authority.free_bytes().unwrap() - 3 * 1024 * 1024);
    let big = vec![7u8; 1024 * 1024];
    assert_eq!(s.put_blob(&a1, &big), cuenca_tree::GitOid::for_blob(&big));
}

/// Requests in flight are bounded; one more is answered 503, and the slot comes back.
#[test]
fn requests_in_flight_are_bounded() {
    let s = Server::start_with(cuenca_server::Limits {
        requests: 2,
        body_idle: std::time::Duration::from_secs(60),
        ..Default::default()
    });
    let a1 = s.mint("a1", Agent, NA, grant(&[], &[]));
    s.whois.node(NA);
    let held = [stalled_upload(&s, &a1), stalled_upload(&s, &a1)];
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(s.get(&a1, "/v1/whoami").status(), 503);
    drop(held);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(s.get(&a1, "/v1/whoami").status(), 200);
}

/// A request is bounded in time, its body included.
#[test]
fn a_request_past_its_deadline_is_answered_408() {
    let s = Server::start_with(cuenca_server::Limits {
        deadline: std::time::Duration::from_millis(500),
        ..Default::default()
    });
    s.add_location("l1");
    let a1 = s.mint("a1", Agent, NA, grant(&["l1"], &[]));
    s.whois.node(NA);
    let mut tcp = send_partial(
        &s,
        format!(
            "POST /v1/forks HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {a1}\r\n\
             Content-Type: application/json\r\nContent-Length: 100\r\n\r\n{{\"fork\""
        ),
    );
    let started = std::time::Instant::now();
    assert!(status_of(&mut tcp).contains("408"));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}
