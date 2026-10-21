//! The authority's shared state, who a request comes from, and what a caller may read.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Free bytes an upload must leave on the state's filesystem.
    min_free: AtomicU64,
    /// Bytes reserved by uploads in flight.
    in_flight: Mutex<u64>,
    /// Blobs each principal has sent the bytes of (see [`Authority::check_puts`]).
    possessed: Mutex<HashMap<PrincipalName, HashSet<GitOid>>>,
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
            min_free: AtomicU64::new(DEFAULT_MIN_FREE),
            in_flight: Mutex::new(0),
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

/// Free space uploads leave on the state's filesystem by default: the log's own writes
/// need room, and no principal's uploads may take it.
pub const DEFAULT_MIN_FREE: u64 = 2 * 1024 * 1024 * 1024;

/// Most blobs remembered per principal; past it the record starts over, and the client
/// sends the bytes again when a draft is refused for a missing blob.
const MAX_POSSESSED: usize = 1 << 20;

impl Authority {
    /// Set the free space an upload must leave ([`DEFAULT_MIN_FREE`]).
    pub fn set_min_free(&self, bytes: u64) {
        self.min_free.store(bytes, Ordering::Relaxed);
    }

    /// Bytes free on the state's filesystem, to an unprivileged writer.
    pub fn free_bytes(&self) -> std::io::Result<u64> {
        let fs = rustix::fs::statvfs(&self.staging)?;
        Ok(fs.f_bavail.saturating_mul(fs.f_frsize))
    }

    /// Reserve room for an upload of `size` bytes, or `None` if it would dip into the
    /// free-space reserve. An upload takes up to twice its size while it is ingested
    /// (the staged file and the store's copy), and every upload in flight holds its
    /// reservation until it ends, so concurrent uploads cannot all pass on the same
    /// free space. Uploads are not counted per principal; this keeps uploads from
    /// stopping the log (and every principal's publishes).
    pub(crate) fn reserve(self: &Arc<Self>, size: u64) -> std::io::Result<Option<Reservation>> {
        let need = size.saturating_mul(2);
        let mut in_flight = self.in_flight.lock().expect("reservation lock");
        let free = self.free_bytes()?;
        if !fits(
            free,
            *in_flight,
            need,
            self.min_free.load(Ordering::Relaxed),
        ) {
            return Ok(None);
        }
        *in_flight += need;
        Ok(Some(Reservation {
            authority: self.clone(),
            bytes: need,
        }))
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

/// Room held for one upload in flight, given back when it is dropped.
pub(crate) struct Reservation {
    authority: Arc<Authority>,
    bytes: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut in_flight = self.authority.in_flight.lock().expect("reservation lock");
        *in_flight = in_flight.saturating_sub(self.bytes);
    }
}

/// Whether `need` more bytes fit in `free` with `in_flight` already promised and
/// `min_free` kept.
fn fits(free: u64, in_flight: u64, need: u64, min_free: u64) -> bool {
    free.checked_sub(in_flight)
        .and_then(|left| left.checked_sub(need))
        .is_some_and(|left| left >= min_free)
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
    /// the fork its principal holds now (on any node), and no other. A former holder
    /// stops reading a fork when it is taken over; it keeps the sibling its stale
    /// drafts go to, which it holds.
    pub fn sees_fork(&self, fork: &Fork) -> bool {
        self.owner_device()
            || self.key.grant.forks.contains(&fork.name)
            || fork
                .holder()
                .is_some_and(|h| h.principal == self.key.principal)
    }

    /// Whether a write to `fork` reaches the core's rules, which decide it: true for
    /// whoever reads the fork, and for a former holder, whose stale draft the rules
    /// keep in a sibling. Anyone else is answered as if the fork did not exist.
    pub fn reaches_fork(&self, fork: &Fork) -> bool {
        self.sees_fork(fork)
            || fork
                .holders
                .iter()
                .any(|h| h.principal == self.key.principal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn room_counts_what_is_in_flight() {
        assert!(fits(100, 0, 50, 50));
        assert!(!fits(100, 0, 51, 50));
        assert!(!fits(100, 10, 50, 50));
        assert!(fits(100, 10, 40, 50));
        assert!(!fits(100, 101, 0, 0));
        assert!(!fits(10, 0, 11, 0));
    }

    #[test]
    fn mapped_v4_is_v4() {
        let v4 = Ipv4Addr::new(100, 82, 195, 109);
        assert_eq!(canonical(IpAddr::V6(v4.to_ipv6_mapped())), IpAddr::V4(v4));
        let v6: Ipv6Addr = "fd7a:115c:a1e0::a937:c36e".parse().unwrap();
        assert_eq!(canonical(IpAddr::V6(v6)), IpAddr::V6(v6));
    }
}
