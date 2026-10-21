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
    assert!(c.names(&c.a1, "/v1/forks").contains(&sibling.to_string()));
    let tree = s.fork_head(&c.a1, sibling);
    assert!(tree.get("offline").is_some());
    // The new holder writes f1 under epoch 2.
    s.whois.node(ND);
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
