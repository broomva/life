//! The authority's shared state, who a request comes from, and what a caller may read.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cuenca_core::{CoreError, Fork, Holder, Location, PrincipalName, Store};
use cuenca_tree::{Changeset, GitOid, Op};

use crate::identity::Whois;
use crate::keys::{KeyRecord, KeyRing, PrincipalKind};

/// Trees whose blob sets are kept in memory for read checks. Trees never change, so
/// an entry is never stale; the cache is only cleared to bound its size.
const BLOB_SET_CACHE: usize = 64;

/// The authority: the store, the keys and the node resolver.
pub struct Authority {
    pub(crate) store: Store,
    pub(crate) keys: KeyRing,
    pub(crate) whois: Arc<dyn Whois>,
    /// Where blob uploads are staged before they are verified.
    pub(crate) staging: PathBuf,
    blob_sets: Mutex<HashMap<GitOid, Arc<HashSet<GitOid>>>>,
    /// Blobs each principal has sent the bytes of (see [`Authority::check_puts`]).
    possessed: Mutex<HashMap<PrincipalName, HashSet<GitOid>>>,
    /// Principals revoked by this process, refused before any record of it is
    /// written ([`Authority::ban`]).
    banned: Mutex<HashSet<PrincipalName>>,
    /// Taken for reading by every agent write from its last check to its commit
    /// ([`Authority::guarded`]), and for writing by [`Authority::ban`].
    revocation: std::sync::RwLock<()>,
}

/// An authenticated request's principal, on the node Tailscale named.
#[derive(Debug, Clone)]
pub struct Caller {
    pub key: KeyRecord,
    pub holder: Holder,
}

/// Why a request was not authenticated. Each is answered 401 without saying which;
/// the reason goes to the authority's log.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no bearer token")]
    NoToken,
    #[error("unknown or wrong key")]
    BadKey,
    #[error("principal {0} is revoked")]
    Revoked(String),
    #[error("tailscale does not name source {0}")]
    UnnamedSource(IpAddr),
    #[error("whois for {0} failed: {1}")]
    WhoisFailed(IpAddr, String),
    #[error("key of {principal} is bound to node {bound}, request came from {actual}")]
    WrongNode {
        principal: String,
        bound: String,
        actual: String,
    },
    #[error("node {0} serves a subnet route")]
    ServesRoutes(String),
    #[error("{0}")]
    Internal(String),
}

impl Authority {
    /// Open the authority's state in `dir`: the store in `dir/store`, the keys file
    /// `dir/keys.json`, staging in `dir/staging`.
    pub fn open(dir: &Path, whois: Arc<dyn Whois>) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let store = Store::open(dir.join("store")).map_err(std::io::Error::other)?;
        let keys = KeyRing::open(dir.join("keys.json"))?;
        // Uploads a crash interrupted are never resumed.
        let staging = dir.join("staging");
        if staging.exists() {
            std::fs::remove_dir_all(&staging)?;
        }
        std::fs::create_dir_all(&staging)?;
        Ok(Self {
            store,
            keys,
            whois,
            staging,
            blob_sets: Mutex::new(HashMap::new()),
            possessed: Mutex::new(HashMap::new()),
            banned: Mutex::new(HashSet::new()),
            revocation: std::sync::RwLock::new(()),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn keys(&self) -> &KeyRing {
        &self.keys
    }

    /// Where uploads are staged before they are verified.
    pub fn staging_dir(&self) -> &Path {
        &self.staging
    }

    /// Who `token`, arriving from `source`, is: the key must verify, its principal must
    /// not be revoked in the log, Tailscale must name the source as the key's node, and
    /// that node must serve no subnet route.
    pub async fn authenticate(
        &self,
        token: Option<&str>,
        source: SocketAddr,
    ) -> Result<Caller, AuthError> {
        let token = token.ok_or(AuthError::NoToken)?;
        let key = self.keys.verify(token).ok_or(AuthError::BadKey)?;
        if self.is_banned(&key.principal) {
            return Err(AuthError::Revoked(key.principal.to_string()));
        }
        if self
            .store
            .revoked(&key.principal)
            .map_err(|e| AuthError::Internal(e.to_string()))?
            .is_some()
        {
            return Err(AuthError::Revoked(key.principal.to_string()));
        }
        let ip = canonical(source.ip());
        let peer = self
            .whois
            .whois(ip)
            .await
            .map_err(|e| AuthError::WhoisFailed(ip, e))?
            .ok_or(AuthError::UnnamedSource(ip))?;
        if peer.node != key.node {
            return Err(AuthError::WrongNode {
                principal: key.principal.to_string(),
                bound: key.node.to_string(),
                actual: peer.node.to_string(),
            });
        }
        if peer.serves_routes {
            return Err(AuthError::ServesRoutes(peer.node.to_string()));
        }
        let holder = Holder::new(key.principal.clone(), key.node.clone());
        Ok(Caller { key, holder })
    }

    /// The blob ids `tree` names, from the cache or the store.
    pub(crate) fn blob_set(&self, tree: GitOid) -> cuenca_core::Result<Arc<HashSet<GitOid>>> {
        if let Some(set) = self.blob_sets.lock().expect("cache lock").get(&tree) {
            return Ok(set.clone());
        }
        let set: Arc<HashSet<GitOid>> = Arc::new(
            self.store
                .tree(tree)?
                .entries()
                .map(|(_, e)| e.oid())
                .collect(),
        );
        let mut cache = self.blob_sets.lock().expect("cache lock");
        if cache.len() >= BLOB_SET_CACHE {
            cache.clear();
        }
        cache.insert(tree, set.clone());
        Ok(set)
    }
}

/// Most blobs remembered per principal; past it the record starts over, and the client
/// sends the bytes again when a draft is refused for a missing blob.
const MAX_POSSESSED: usize = 1 << 20;

impl Authority {
    /// Refuse `principal` from now on, in this process: the part of revocation that
    /// needs no write, so it holds even when the disk is full. The admin socket's
    /// `key_revoke` does this first, then records the revocation in the log.
    pub fn ban(&self, principal: &PrincipalName) {
        // Refused at once, reads included; then wait for writes already past their
        // check, so once this returns no write by `principal` is still to commit.
        self.banned
            .lock()
            .expect("ban lock")
            .insert(principal.clone());
        drop(self.revocation.write().expect("revocation lock"));
    }

    /// Run `write` for `principal` only if it is not revoked, checked again here, at
    /// the end of a request that was authenticated at its start: a ban (or a recorded
    /// revocation) either lands before this check and refuses the write, or waits for
    /// the write to commit.
    pub(crate) fn guarded<T, E: From<CoreError>>(
        &self,
        principal: &PrincipalName,
        write: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let _writes = self.revocation.read().expect("revocation lock");
        if self.is_banned(principal) || self.store.revoked(principal)?.is_some() {
            return Err(CoreError::Revoked(principal.to_string()).into());
        }
        write()
    }

    /// Whether this process has refused `principal` ([`Authority::ban`]).
    pub fn is_banned(&self, principal: &PrincipalName) -> bool {
        self.banned.lock().expect("ban lock").contains(principal)
    }

    /// Remember that `principal` sent the bytes of `oid`, verified.
    pub(crate) fn possesses(&self, principal: &PrincipalName, oid: GitOid) {
        let mut all = self.possessed.lock().expect("possession lock");
        let set = all.entry(principal.clone()).or_default();
        if set.len() >= MAX_POSSESSED {
            set.clear();
        }
        set.insert(oid);
    }

    /// Whether `caller` may name each blob `changeset` puts, in a draft of `fork`.
    ///
    /// The store accepts any blob it holds, whoever sent it, and a blob in a fork the
    /// caller holds can be read back. So a put must name a blob the caller can already
    /// read, or has shown it has:
    ///
    /// - a caller who reads the fork may name a blob in the fork's head, the tree the
    ///   API serves it;
    /// - any caller may name a blob whose bytes its principal uploaded to this
    ///   authority since it started.
    ///
    /// A former holder (whose draft the rules fence into its sibling) reads the fork no
    /// more, so it gets the second rule only. Anything else is refused as
    /// `MissingBlob`, the same answer an absent blob gets, so the refusal does not say
    /// whether the store holds it. This runs before the rules, so after a restart even
    /// a retried draft is refused this way until the client sends the bytes again.
    pub(crate) fn check_puts(
        &self,
        caller: &Caller,
        fork: &Fork,
        changeset: &Changeset,
    ) -> cuenca_core::Result<()> {
        let head = if caller.sees_fork(fork) {
            Some(self.blob_set(fork.head)?)
        } else {
            None
        };
        let possessed = self.possessed.lock().expect("possession lock");
        let own = possessed.get(&caller.key.principal);
        for op in &changeset.ops {
            if let Op::Put { entry, .. } = op {
                let oid = entry.oid();
                let known = head.as_ref().is_some_and(|set| set.contains(&oid))
                    || own.is_some_and(|set| set.contains(&oid));
                if !known {
                    return Err(CoreError::MissingBlob(oid));
                }
            }
        }
        Ok(())
    }
}

/// An IPv4 address that arrived as IPv4-mapped IPv6 is the IPv4 address.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

impl Caller {
    fn owner_device(&self) -> bool {
        self.key.kind == PrincipalKind::OwnerDevice
    }

    /// Main of a location: an owner device reads every one; an agent, those it is
    /// granted, which are also the ones it may fork.
    pub fn sees_location(&self, location: &Location) -> bool {
        self.owner_device() || self.key.grant.locations.contains(&location.name)
    }

    /// A fork: an owner device reads every one; an agent, the forks its grant names and
    /// the fork it holds now (its principal on its key's node: a holder, as the core
    /// defines one), and no other. A former holder
    /// stops reading a fork when it is taken over; it keeps the sibling its stale
    /// drafts go to, which it holds.
    pub fn sees_fork(&self, fork: &Fork) -> bool {
        self.owner_device()
            || self.key.grant.forks.contains(&fork.name)
            || fork.holder() == Some(&self.holder)
    }

    /// Whether a write to `fork` reaches the core's rules, which decide it: true for
    /// whoever reads the fork, and for a former holder, whose stale draft the rules
    /// keep in a sibling. Anyone else is answered as if the fork did not exist.
    pub fn reaches_fork(&self, fork: &Fork) -> bool {
        self.sees_fork(fork) || fork.holders.contains(&self.holder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    struct Nobody;

    impl crate::Whois for Nobody {
        fn whois(&self, _ip: IpAddr) -> crate::WhoisFuture<'_> {
            Box::pin(async { Ok(None) })
        }
    }

    /// A ban waits for a write already past its check, so once `ban` returns no write
    /// by that principal is still to commit.
    #[test]
    fn a_ban_waits_for_a_write_past_its_check() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(Authority::open(dir.path(), Arc::new(Nobody)).unwrap());
        let p = PrincipalName::new("a1").unwrap();
        let (inside, reached) = mpsc::channel();
        let (go, wait) = mpsc::channel::<()>();
        let writer = {
            let (a, p) = (a.clone(), p.clone());
            std::thread::spawn(move || {
                a.guarded(&p, || {
                    inside.send(()).unwrap();
                    wait.recv().unwrap();
                    Ok::<_, CoreError>(())
                })
            })
        };
        reached.recv().unwrap();
        let banner = {
            let (a, p) = (a.clone(), p.clone());
            std::thread::spawn(move || a.ban(&p))
        };
        std::thread::sleep(Duration::from_millis(200));
        assert!(a.is_banned(&p), "refused at once");
        assert!(
            !banner.is_finished(),
            "the ban must wait for the write to commit"
        );
        go.send(()).unwrap();
        writer.join().unwrap().unwrap();
        banner.join().unwrap();
        // And a write after the ban is refused.
        assert!(a.guarded(&p, || Ok::<_, CoreError>(())).is_err());
    }

    #[test]
    fn mapped_v4_is_v4() {
        let v4 = Ipv4Addr::new(100, 82, 195, 109);
        assert_eq!(canonical(IpAddr::V6(v4.to_ipv6_mapped())), IpAddr::V4(v4));
        let v6: Ipv6Addr = "fd7a:115c:a1e0::a937:c36e".parse().unwrap();
        assert_eq!(canonical(IpAddr::V6(v6)), IpAddr::V6(v6));
    }
}
