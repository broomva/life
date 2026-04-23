//! Network isolation types for egress metering.
//!
//! Distinct from [`crate::sandbox::NetworkPolicy`] (which is policy
//! *declaration*). This module holds the record types reported by a VM's
//! network hook and consumed by the (future) `NetworkIsolationPort` trait
//! (lands in BRO-849).
//!
//! BRO-847 seeds only [`EgressTarget`] and [`EgressProtocol`]; the trait
//! and enforcement impls arrive later.

use serde::{Deserialize, Serialize};

/// An egress destination observed by the VM's network hook. Used for
/// metering and for emitting `network.egress` audit events.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EgressTarget {
    /// Destination host (hostname or IP literal).
    pub host: String,
    /// Destination port.
    pub port: u16,
    /// L4 protocol used.
    pub protocol: EgressProtocol,
}

/// Layer-4 protocol family for an observed egress flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EgressProtocol {
    Tcp,
    Udp,
    Icmp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egress_target_roundtrip() {
        let t = EgressTarget {
            host: "api.example.com".into(),
            port: 443,
            protocol: EgressProtocol::Tcp,
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: EgressTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn egress_protocol_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&EgressProtocol::Tcp).unwrap(),
            "\"tcp\""
        );
        let back: EgressProtocol = serde_json::from_str("\"udp\"").unwrap();
        assert_eq!(back, EgressProtocol::Udp);
        let back: EgressProtocol = serde_json::from_str("\"icmp\"").unwrap();
        assert_eq!(back, EgressProtocol::Icmp);
    }

    #[test]
    fn egress_target_hashable() {
        use std::collections::HashSet;
        let t = EgressTarget {
            host: "x".into(),
            port: 1,
            protocol: EgressProtocol::Tcp,
        };
        let mut s = HashSet::new();
        s.insert(t.clone());
        assert!(s.contains(&t));
    }
}
