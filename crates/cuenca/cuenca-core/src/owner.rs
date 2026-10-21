//! Who holds a fork (spec section 2a, "One writer per fork").

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::names::{ForkName, NameError, PrincipalName};

/// Longest node id, in bytes.
pub const MAX_NODE_BYTES: usize = 64;

/// The machine a principal writes from: the Tailscale stable node ID that
/// `tailscale whois` names for a request's source (an opaque ASCII token such as
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
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if ok {
            Ok(Self(s))
        } else {
            Err(NameError::new(
                "node",
                s,
                "1-64 bytes of A-Z, a-z, 0-9, '_' or '-'",
            ))
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

/// The sibling fork a publish from `node` under a stale epoch of `fork` is kept in:
/// `<fork>-from-<node>`, the node lowercased so the name is a valid fork name. Two
/// node ids that differ only in case name the same sibling; the rules then refuse
/// the second node's publish rather than mix it in (the sibling has another holder).
pub fn sibling_name(fork: &ForkName, node: &NodeId) -> Result<ForkName, NameError> {
    ForkName::new(format!(
        "{fork}-from-{}",
        node.as_str().to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids() {
        for ok in ["nD8HyGmA8Q11CNTRL", "n1", "a-b_c", &"x".repeat(64)] {
            assert!(NodeId::new(ok).is_ok(), "{ok}");
        }
        for bad in ["", "a b", "a/b", "a.b", "é", &"x".repeat(65)] {
            assert!(NodeId::new(bad).is_err(), "{bad:?}");
        }
        assert!(serde_json::from_str::<NodeId>("\"../x\"").is_err());
    }

    #[test]
    fn siblings_are_named_for_the_node() {
        let f = ForkName::new("feat").unwrap();
        let n = NodeId::new("nAbC_1-CNTRL").unwrap();
        assert_eq!(
            sibling_name(&f, &n).unwrap().as_str(),
            "feat-from-nabc_1-cntrl"
        );
        // A sibling of a long fork from a long node may not fit in a name.
        let long = ForkName::new("f".repeat(100)).unwrap();
        assert!(sibling_name(&long, &NodeId::new("n".repeat(30)).unwrap()).is_err());
    }
}
