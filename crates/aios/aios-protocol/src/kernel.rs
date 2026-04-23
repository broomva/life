//! Kernel-tier types: attribution, context, gate kinds, and the richer
//! kernel-tier error surface.
//!
//! These types are consumed by the (future) `KernelPort` trait (lands in
//! BRO-849) and emitted as payloads on `kernel.*`
//! [`crate::event::EventKind`] variants. This module holds only types — no
//! traits land here in BRO-847.
//!
//! ## Error naming
//!
//! The crate keeps the legacy [`crate::error::KernelError`] as the crate-root
//! re-export for backward compatibility. A richer, kernel-tier
//! [`KernelError`] (this module) carries typed gate and backend variants.
//! The richer error is intentionally NOT re-exported at the crate root in
//! BRO-847 to avoid shadowing the legacy error; reach it via
//! `aios_protocol::kernel::KernelError`. The migration sweep that moves all
//! downstream crates to the richer error is scheduled for BRO-856.

use serde::{Deserialize, Serialize};

/// Identifies a wallet for on-chain attribution of kernel-emitted events.
///
/// The `address` format is chain-dependent (0x… hex for EVM chains,
/// base58 for Solana, bech32 for Cosmos, etc.). The kernel does not
/// validate the format — backends and downstream gates do.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WalletAttribution {
    pub address: String,
    pub chain: ChainId,
}

/// Chain identifier for the wallet's settlement network.
///
/// Follows CAIP-2 (`<namespace>:<reference>`) format. Helpers are provided
/// for the chains Haima actively supports; other chains can be constructed
/// via [`ChainId::from_caip2`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChainId(pub String);

impl ChainId {
    /// Base L2 mainnet — Haima's primary settlement chain.
    pub fn base() -> Self {
        Self("eip155:8453".into())
    }

    /// Ethereum mainnet.
    pub fn ethereum() -> Self {
        Self("eip155:1".into())
    }

    /// Construct from a raw CAIP-2 string (e.g. `"eip155:10"` for Optimism).
    pub fn from_caip2(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// View the CAIP-2 string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_attribution_roundtrip() {
        let w = WalletAttribution {
            address: "0xabcdef".into(),
            chain: ChainId::base(),
        };
        let json = serde_json::to_string(&w).unwrap();
        let back: WalletAttribution = serde_json::from_str(&json).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn chain_id_helpers() {
        assert_eq!(ChainId::base().0, "eip155:8453");
        assert_eq!(ChainId::ethereum().0, "eip155:1");
    }

    #[test]
    fn chain_id_is_transparent() {
        let c = ChainId::base();
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"eip155:8453\"");
        let back: ChainId = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn chain_id_from_caip2() {
        let optimism = ChainId::from_caip2("eip155:10");
        assert_eq!(optimism.as_str(), "eip155:10");
    }
}
