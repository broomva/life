#![allow(dead_code)]

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cuenca_core::NodeId;
use cuenca_server::admin::{self, AdminRequest};
use cuenca_server::{Authority, Grant, Peer, PrincipalKind, Whois, WhoisFuture};
use cuenca_tree::{Changeset, Entry, GitOid, Op, RepoPath, Tree};
use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};

/// A [`Whois`] whose answer the test sets: every source is named as `answer`.
pub struct Switch(Mutex<Result<Option<Peer>, String>>);

impl Switch {
    pub fn set(&self, answer: Result<Option<Peer>, String>) {
        *self.0.lock().unwrap() = answer;
    }

    /// Name every source as `node`, serving no route.
    pub fn node(&self, node: &str) {
        self.set(Ok(Some(peer(node, false))));
    }
}

impl Whois for Switch {
    fn whois(&self, _ip: IpAddr) -> WhoisFuture<'_> {
        let answer = self.0.lock().unwrap().clone();
        Box::pin(async move { answer })
    }
}

pub fn peer(node: &str, serves_routes: bool) -> Peer {
    Peer {
        node: NodeId::new(node).unwrap(),
        serves_routes,
    }
}

/// An authority on 127.0.0.1 (an unsafe bind, for tests), on a runtime of its own.
pub struct Server {
    pub url: String,
    pub admin: PathBuf,
    pub whois: Arc<Switch>,
    pub authority: Arc<Authority>,
    pub client: Client,
    rt: tokio::runtime::Runtime,
    _dir: Option<tempfile::TempDir>,
}

impl Server {
    pub fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let mut s = Self::start_in(&path);
        s._dir = Some(dir);
        s
    }

    /// Start on the state in `dir` (kept by the caller).
    pub fn start_in(dir: &Path) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let whois = Arc::new(Switch(Mutex::new(Ok(None))));
        let authority =
            Arc::new(Authority::open(&dir.join("state"), whois.clone() as Arc<dyn Whois>).unwrap());
        // The admin socket's directory must be its owner's alone, whatever the umask.
        std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let admin_path = dir.join("admin.sock");
        let (listener, admin_listener) = rt.block_on(async {
            (
                tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
                admin::bind(&admin_path).unwrap(),
            )
        });
        let addr: SocketAddr = listener.local_addr().unwrap();
        rt.spawn(cuenca_server::serve_http(listener, authority.clone()));
        rt.spawn(admin::serve(admin_listener, authority.clone()));
        Self {
            url: format!("http://{addr}"),
            admin: admin_path,
            whois,
            authority,
            client: Client::new(),
            rt,
            _dir: None,
        }
    }

    pub fn admin(&self, req: AdminRequest) -> Value {
        admin::call(&self.admin, &req).unwrap()
    }

    /// Mint a key and return its token.
    pub fn mint(&self, principal: &str, kind: PrincipalKind, node: &str, grant: Grant) -> String {
        let v = self.admin(AdminRequest::KeyMint {
            principal: principal.parse_name(),
            kind,
            node: NodeId::new(node).unwrap(),
            grant,
        });
        assert_eq!(v["ok"], true, "{v}");
        v["token"].as_str().unwrap().to_string()
    }

    pub fn add_location(&self, name: &str) {
        let v = self.admin(AdminRequest::LocationAdd {
            name: cuenca_core::LocationName::new(name).unwrap(),
            remote: None,
            git: None,
        });
        assert_eq!(v["ok"], true, "{v}");
    }

    pub fn get(&self, token: &str, path: &str) -> Response {
        self.client
            .get(format!("{}{path}", self.url))
            .bearer_auth(token)
            .send()
            .unwrap()
    }

    pub fn post(&self, token: &str, path: &str, body: Value) -> Response {
        self.client
            .post(format!("{}{path}", self.url))
            .bearer_auth(token)
            .json(&body)
            .send()
            .unwrap()
    }

    /// Upload `content` and return its git id.
    pub fn put_blob(&self, token: &str, content: &[u8]) -> GitOid {
        let oid = GitOid::for_blob(content);
        let r = self
            .client
            .put(format!("{}/v1/blobs/{}", self.url, oid.to_hex()))
            .bearer_auth(token)
            .body(content.to_vec())
            .send()
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().unwrap());
        oid
    }

    pub fn fork_head(&self, token: &str, fork: &str) -> Tree {
        let v: Value = self
            .get(token, &format!("/v1/forks/{fork}/tree"))
            .json()
            .unwrap();
        serde_json::from_value(v["tree"].clone()).unwrap()
    }

    /// The changeset writing `files` over `base`, uploading their content.
    pub fn write(&self, token: &str, base: &Tree, files: &[(&str, &[u8])]) -> Changeset {
        let ops = files
            .iter()
            .map(|(p, c)| {
                Op::put(
                    RepoPath::new(*p).unwrap(),
                    Entry::File(self.put_blob(token, c)),
                )
            })
            .collect();
        Changeset::new(base, ops).unwrap()
    }

    pub fn publish(&self, token: &str, fork: &str, epoch: u64, cs: &Changeset) -> Response {
        self.post(
            token,
            &format!("/v1/forks/{fork}/drafts"),
            json!({ "epoch": epoch, "changeset": cs }),
        )
    }

    /// Drop the server without waiting for in-flight requests.
    pub fn stop(self) {
        self.rt.shutdown_background();
    }
}

pub trait ParseName {
    fn parse_name(&self) -> cuenca_core::PrincipalName;
}

impl ParseName for str {
    fn parse_name(&self) -> cuenca_core::PrincipalName {
        cuenca_core::PrincipalName::new(self).unwrap()
    }
}

pub fn grant(locations: &[&str], forks: &[&str]) -> Grant {
    Grant {
        locations: locations
            .iter()
            .map(|l| cuenca_core::LocationName::new(*l).unwrap())
            .collect(),
        forks: forks
            .iter()
            .map(|f| cuenca_core::ForkName::new(*f).unwrap())
            .collect(),
    }
}
