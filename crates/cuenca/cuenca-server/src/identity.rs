//! Which tailnet node a request comes from (spec section 0).
//!
//! The authority asks Tailscale, on every request, which node owns the request's
//! source address (`tailscale whois`), and whether that node serves any subnet route
//! (its `PrimaryRoutes`, read from the authority's own netmap, `tailscale status`).
//! Traffic through a subnet router arrives as the router, so one router address can
//! stand for many devices: a node serving a route is refused, including one that
//! started serving it after its key was minted. A source whois cannot name is refused.

use std::future::Future;
use std::net::IpAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use cuenca_core::NodeId;
use serde::Deserialize;

/// What Tailscale says about a request's source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub node: NodeId,
    /// Whether the node serves any subnet route, by whois or by the netmap.
    pub serves_routes: bool,
}

pub type WhoisFuture<'a> = Pin<Box<dyn Future<Output = Result<Option<Peer>, String>> + Send + 'a>>;

/// Names the node behind a source address. `Ok(None)`: nobody on the tailnet has
/// that address. `Err`: the answer could not be had. The authority refuses both.
pub trait Whois: Send + Sync + 'static {
    fn whois(&self, ip: IpAddr) -> WhoisFuture<'_>;
}

/// [`Whois`] through the `tailscale` CLI of the authority's host.
pub struct Tailscale {
    bin: PathBuf,
}

const CALL_TIMEOUT: Duration = Duration::from_secs(5);

impl Tailscale {
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Self { bin: bin.into() }
    }

    /// Run `tailscale <args>`; `None` when it exits non-zero.
    async fn call(&self, args: &[&str]) -> Result<Option<Vec<u8>>, String> {
        let run = tokio::process::Command::new(&self.bin)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(CALL_TIMEOUT, run)
            .await
            .map_err(|_| format!("tailscale {} timed out", args[0]))?
            .map_err(|e| format!("tailscale {}: {e}", args[0]))?;
        Ok(out.status.success().then_some(out.stdout))
    }
}

#[derive(Deserialize)]
struct WhoisJson {
    #[serde(rename = "Node")]
    node: NodeJson,
}

#[derive(Deserialize)]
struct NodeJson {
    #[serde(rename = "StableID")]
    stable_id: String,
    #[serde(rename = "PrimaryRoutes", default)]
    primary_routes: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct StatusJson {
    #[serde(rename = "Self")]
    me: Option<PeerJson>,
    #[serde(rename = "Peer", default)]
    peers: Option<std::collections::HashMap<String, PeerJson>>,
}

#[derive(Deserialize)]
struct PeerJson {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "PrimaryRoutes", default)]
    primary_routes: Option<Vec<String>>,
}

/// Parse `tailscale whois --json` and `tailscale status --json` into a [`Peer`]. A
/// node absent from the netmap is not named.
fn peer_from(whois: &[u8], status: &[u8]) -> Result<Option<Peer>, String> {
    let whois: WhoisJson = serde_json::from_slice(whois).map_err(|e| format!("whois: {e}"))?;
    let status: StatusJson = serde_json::from_slice(status).map_err(|e| format!("status: {e}"))?;
    let Ok(node) = NodeId::new(whois.node.stable_id.clone()) else {
        return Err(format!(
            "whois named {:?}, not a stable node id",
            whois.node.stable_id
        ));
    };
    let in_netmap = status
        .me
        .iter()
        .chain(status.peers.iter().flat_map(|p| p.values()))
        .find(|p| p.id == whois.node.stable_id);
    let Some(entry) = in_netmap else {
        return Ok(None);
    };
    let routes = |r: &Option<Vec<String>>| r.as_ref().is_some_and(|r| !r.is_empty());
    Ok(Some(Peer {
        node,
        serves_routes: routes(&whois.node.primary_routes) || routes(&entry.primary_routes),
    }))
}

impl Whois for Tailscale {
    fn whois(&self, ip: IpAddr) -> WhoisFuture<'_> {
        Box::pin(async move {
            let ip = ip.to_string();
            let Some(whois) = self.call(&["whois", "--json", &ip]).await? else {
                return Ok(None);
            };
            let status = self
                .call(&["status", "--json"])
                .await?
                .ok_or("tailscale status failed")?;
            peer_from(&whois, &status)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHOIS: &str = r#"{"Node":{"ID":1,"StableID":"nXEG5Qhwed11CNTRL","Addresses":["100.82.195.109/32"]},"UserProfile":{}}"#;
    const WHOIS_ROUTES: &str =
        r#"{"Node":{"StableID":"nXEG5Qhwed11CNTRL","PrimaryRoutes":["192.168.1.0/24"]}}"#;
    const STATUS: &str =
        r#"{"Self":{"ID":"nSelf11CNTRL"},"Peer":{"nodekey:a":{"ID":"nXEG5Qhwed11CNTRL"}}}"#;
    const STATUS_ROUTES: &str = r#"{"Self":{"ID":"nSelf11CNTRL"},"Peer":{"nodekey:a":{"ID":"nXEG5Qhwed11CNTRL","PrimaryRoutes":["10.0.0.0/8"]}}}"#;

    fn node() -> NodeId {
        NodeId::new("nXEG5Qhwed11CNTRL").unwrap()
    }

    #[test]
    fn a_peer_in_the_netmap_is_named() {
        let peer = peer_from(WHOIS.as_bytes(), STATUS.as_bytes()).unwrap();
        assert_eq!(
            peer,
            Some(Peer {
                node: node(),
                serves_routes: false
            })
        );
    }

    #[test]
    fn routes_from_either_source_mark_the_peer() {
        for (w, s) in [(WHOIS_ROUTES, STATUS), (WHOIS, STATUS_ROUTES)] {
            let peer = peer_from(w.as_bytes(), s.as_bytes()).unwrap().unwrap();
            assert!(peer.serves_routes, "{w} {s}");
        }
        let empty = r#"{"Node":{"StableID":"nXEG5Qhwed11CNTRL","PrimaryRoutes":[]}}"#;
        let peer = peer_from(empty.as_bytes(), STATUS.as_bytes())
            .unwrap()
            .unwrap();
        assert!(!peer.serves_routes);
    }

    #[test]
    fn the_authority_itself_is_found_under_self() {
        let w = r#"{"Node":{"StableID":"nSelf11CNTRL"}}"#;
        let peer = peer_from(w.as_bytes(), STATUS.as_bytes()).unwrap().unwrap();
        assert_eq!(peer.node.as_str(), "nSelf11CNTRL");
    }

    #[test]
    fn a_node_missing_from_the_netmap_or_garbage_is_not_named() {
        let w = r#"{"Node":{"StableID":"nOther11CNTRL"}}"#;
        assert_eq!(peer_from(w.as_bytes(), STATUS.as_bytes()).unwrap(), None);
        assert!(peer_from(b"{}", STATUS.as_bytes()).is_err());
        assert!(peer_from(WHOIS.as_bytes(), b"nope").is_err());
        let bad_id = r#"{"Node":{"StableID":"../x"}}"#;
        assert!(peer_from(bad_id.as_bytes(), STATUS.as_bytes()).is_err());
    }
}
