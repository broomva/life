//! Who holds a fork (spec section 2a, "One writer per fork").

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::names::{ForkName, MAX_NAME_BYTES, NameError, PrincipalName};

/// Longest node id, in bytes.
pub const MAX_NODE_BYTES: usize = 64;

/// The machine a principal writes from: the Tailscale stable node ID that
/// `tailscale whois` names for a request's source (an opaque alphanumeric token such as
/// `nD8HyGmA8Q11CNTRL`). Checking it against the request is the authority's job
/// (BRO-3251); here it is a name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct NodeId(String);

impl NodeId {
    pub fn new(s: impl Into<String>) -> Result<Self, NameError> {
        let s = s.into();
        let ok = !s.is_empty()
            && s.len() <= MAX_NODE_BYTES
            && s.bytes().all(|b| b.is_ascii_alphanumeric());
        if ok {
            Ok(Self(s))
        } else {
            Err(NameError::new("node", s, "1-64 bytes of A-Z, a-z or 0-9"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// A fork's writer: a principal, writing from one machine. The same principal on
/// another machine is another holder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Holder {
    pub principal: PrincipalName,
    pub node: NodeId,
}

impl Holder {
    pub fn new(principal: PrincipalName, node: NodeId) -> Self {
        Self { principal, node }
    }
}

impl fmt::Display for Holder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.principal, self.node)
    }
}

/// Who asks to write a fork, and the fencing epoch they hold it under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Writer {
    pub holder: Holder,
    pub epoch: u64,
}

impl Writer {
    pub fn new(holder: Holder, epoch: u64) -> Self {
        Self { holder, epoch }
    }
}

/// Bytes of the parent's name kept in a hashed sibling name.
const HASHED_PREFIX_BYTES: usize = 80;

/// The sibling fork that keeps drafts published to `parent` under its stale epoch
/// `epoch`, by `node` (the node that held `parent` under that epoch).
///
/// `(parent, epoch)` names exactly one former holder, so each stale holder and epoch
/// gets a sibling of its own. The name is `<parent>-from-<node>-e<epoch>`, the node
/// lowercased for readability only (node ids are alphanumeric, so the name still
/// parses back to `parent` and `epoch` uniquely). When that would not fit in a fork
/// name, it is `<first 80 bytes of parent>-from.<32 hex>`, the hex the first 128 bits
/// of the SHA-256 of the parent's whole name and the epoch: unique short of a hash
/// collision, which the rules refuse rather than mix. The two shapes cannot coincide
/// (one ends in `-e<digits>`, the other in `.<hex>`), and fork creation refuses any
/// name containing `-from` ([`crate::CoreError::ReservedName`]), so a sibling's name is
/// never taken by anything else.
pub fn sibling_name(parent: &ForkName, epoch: u64, node: &NodeId) -> ForkName {
    let readable = format!(
        "{parent}-from-{}-e{epoch}",
        node.as_str().to_ascii_lowercase()
    );
    if readable.len() <= MAX_NAME_BYTES {
        return ForkName::new(readable).expect("a readable sibling name is a valid name");
    }
    let mut hash = Sha256::new();
    hash.update(parent.as_str().as_bytes());
    hash.update([0]);
    hash.update(epoch.to_string().as_bytes());
    let digest = hash.finalize();
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    let keep = parent.as_str().len().min(HASHED_PREFIX_BYTES);
    ForkName::new(format!("{}-from.{hex}", &parent.as_str()[..keep]))
        .expect("a hashed sibling name is a valid name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids() {
        for ok in ["nD8HyGmA8Q11CNTRL", "n1", &"x".repeat(64)] {
            assert!(NodeId::new(ok).is_ok(), "{ok}");
        }
        for bad in ["", "a b", "a/b", "a.b", "a-b", "a_b", "é", &"x".repeat(65)] {
            assert!(NodeId::new(bad).is_err(), "{bad:?}");
        }
        assert!(serde_json::from_str::<NodeId>("\"../x\"").is_err());
    }

    #[test]
    fn siblings_are_named_for_the_node_and_epoch() {
        let f = ForkName::new("feat").unwrap();
        let n = NodeId::new("nAbC1CNTRL").unwrap();
        assert_eq!(sibling_name(&f, 3, &n).as_str(), "feat-from-nabc1cntrl-e3");
    }

    /// Every (parent, epoch) gets a name of its own that fits, whatever the node, down
    /// three levels of siblings of siblings, across both name shapes.
    #[test]
    fn a_sibling_name_always_fits_and_never_names_two_siblings() {
        let nodes = [
            NodeId::new("n").unwrap(),
            NodeId::new("N").unwrap(),
            NodeId::new("n".repeat(64)).unwrap(),
        ];
        let mut parents = vec![
            ForkName::new("f").unwrap(),
            ForkName::new("f".repeat(57)).unwrap(),
            ForkName::new("f".repeat(58)).unwrap(),
            ForkName::new("a.".repeat(64)).unwrap(),
            ForkName::new("f".repeat(MAX_NAME_BYTES)).unwrap(),
        ];
        let mut owner = std::collections::HashMap::new();
        for _depth in 0..3 {
            let mut next = Vec::new();
            for p in &parents {
                for epoch in [1, 2, 10, u64::MAX] {
                    for node in &nodes {
                        let s = sibling_name(p, epoch, node);
                        assert!(s.as_str().len() <= MAX_NAME_BYTES, "{s}");
                        assert!(s.as_str().contains("-from"), "{s}");
                        let key = (p.clone(), epoch);
                        assert_eq!(owner.entry(s.clone()).or_insert(key.clone()), &key, "{s}");
                        next.push(s);
                    }
                }
            }
            parents = next;
        }
        assert!(owner.keys().any(|s| s.as_str().contains("-from.")));
        // Long names take the hashed shape, which keeps a prefix of the parent.
        let long = ForkName::new("f".repeat(100)).unwrap();
        let s = sibling_name(&long, 1, &nodes[2]);
        assert!(s.as_str().starts_with(&"f".repeat(80)) && s.as_str().contains("-from."));
        assert_ne!(s, sibling_name(&long, 2, &nodes[2]));
        // The parent's name and the epoch are hashed apart: "x..x1" at epoch 2 is not
        // "x..x" at epoch 12.
        let one = ForkName::new(format!("{}1", "x".repeat(99))).unwrap();
        let other = ForkName::new("x".repeat(99)).unwrap();
        assert_ne!(
            sibling_name(&one, 2, &nodes[2]),
            sibling_name(&other, 12, &nodes[2])
        );
    }
}
