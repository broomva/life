//! The authority's shared state, who a request comes from, and what a caller may read.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cuenca_core::{Fork, Holder, Location, Store};
use cuenca_tree::GitOid;

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
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging)?;
        Ok(Self {
            store,
            keys,
            whois,
            staging,
            blob_sets: Mutex::new(HashMap::new()),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn keys(&self) -> &KeyRing {
        &self.keys
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
    /// the forks its principal holds or held (on any node), and no other.
    pub fn sees_fork(&self, fork: &Fork) -> bool {
        self.owner_device()
            || self.key.grant.forks.contains(&fork.name)
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
    fn mapped_v4_is_v4() {
        let v4 = Ipv4Addr::new(100, 82, 195, 109);
        assert_eq!(canonical(IpAddr::V6(v4.to_ipv6_mapped())), IpAddr::V4(v4));
        let v6: Ipv6Addr = "fd7a:115c:a1e0::a937:c36e".parse().unwrap();
        assert_eq!(canonical(IpAddr::V6(v6)), IpAddr::V6(v6));
    }
}
