//! Anima-backed implementation of ergon attestation traits.
//!
//! See `docs/architecture/adr/2026-05-22-anima-signing-surface-for-ergon-attestation.md`
//! (BRO-1226) for the design rationale + open questions.
//!
//! This crate ships the **skeleton only** — both `sign_step_receipt`
//! and the two `SoulAttester` methods are unimplemented and return
//! `Err(...)` pointing back at the ADR. The implementation lands in a
//! follow-up ticket once Open Questions §1-3 (HookCtx access, journal
//! abstraction, canonical-JSON serializer) are resolved on review.

use std::sync::Arc;

use anima_identity::custody::AnimaCustody;
use async_trait::async_trait;
use ergon::SessionId;
use ergon_life_hooks::SoulAttester;
use serde_json::Value;

/// Per-step attestation signer.
///
/// New trait — extends ergon attestation past session-boundary
/// (`SoulAttester`) to per-step receipts. The receipt body shape is
/// canonical-JSON per ADR §4; the returned string is the compact JWS
/// (`<header>.<body>.<signature>`) signed by the agent's custody key.
#[async_trait]
pub trait AgentAttestationSigner: Send + Sync {
    /// Sign a step receipt. The receipt is opaque to this trait — the
    /// adapter chooses how to canonicalize before signing. Errors are
    /// non-fatal at the hook layer (same contract as `SoulAttester`).
    async fn sign_step_receipt(
        &self,
        receipt: &Value,
    ) -> std::result::Result<String, String>;
}

/// Adapter — wraps an `Arc<dyn AnimaCustody>` and implements both
/// `SoulAttester` (session-boundary attestation, existing) and
/// `AgentAttestationSigner` (per-step receipts, new).
///
/// See ADR §2 for the custody-abstraction rationale. The 6 production
/// `AnimaCustody` backends (Vault / softhsm / WebCrypto / RemoteAnima /
/// HardwareWalletAnima / VaultTransitAnima) remain entirely behind this
/// `Arc<dyn ...>` — the adapter neither knows nor cares which is in use.
pub struct AgentAttestationAdapter {
    custody: Arc<dyn AnimaCustody>,
}

impl AgentAttestationAdapter {
    /// Construct from an `AnimaCustody` handle. Typically called once
    /// at workflow-runner startup; the same adapter is shared across
    /// all hook invocations for the agent's lifetime.
    pub fn new(custody: Arc<dyn AnimaCustody>) -> Self {
        Self { custody }
    }

    /// Agent's DID (the `kid` that lands in every signed JWS header).
    pub fn agent_did(&self) -> &str {
        self.custody.user_did()
    }
}

#[async_trait]
impl AgentAttestationSigner for AgentAttestationAdapter {
    async fn sign_step_receipt(
        &self,
        _receipt: &Value,
    ) -> std::result::Result<String, String> {
        // Implementation follow-up tracked in BRO-1226 implementation
        // ticket (filed after the ADR review pass).
        //
        // The implementation will:
        //   1. Serialize the receipt with canonical JSON (RFC 8785 — see
        //      ADR Open §3).
        //   2. Call self.custody.sign_jws(canonical_json) → compact JWS.
        //   3. Return the JWS string.
        //   4. On AnimaCustody::sign_jws error: log + return Err(msg).
        Err(format!(
            "AgentAttestationAdapter::sign_step_receipt not yet implemented \
             (did={}); see ADR §3-§4",
            self.custody.user_did()
        ))
    }
}

#[async_trait]
impl SoulAttester for AgentAttestationAdapter {
    async fn sign_session_start(
        &self,
        _session_id: &SessionId,
        _workflow_name: &str,
    ) -> std::result::Result<(), String> {
        // Implementation follow-up. Will build a session-boundary
        // receipt {kind: "session_start", session, workflow, iat,
        // agent_did} and call self.custody.sign_jws + emit on journal.
        Err("SoulAttester::sign_session_start not yet implemented; see ADR §4".into())
    }

    async fn sign_session_end(
        &self,
        _session_id: &SessionId,
        _workflow_name: &str,
        _ok: bool,
    ) -> std::result::Result<(), String> {
        // Implementation follow-up. Symmetric with sign_session_start;
        // adds {kind: "session_end", ok} to the receipt body.
        Err("SoulAttester::sign_session_end not yet implemented; see ADR §4".into())
    }
}

#[cfg(test)]
mod tests {
    // Adapter unit tests deferred to the implementation ticket — until
    // the methods do real work, there's no useful behavior to assert.
    // The skeleton compiles and exposes the public surface; that is
    // what acceptance §3 of the ADR requires.

    use super::*;

    /// Compile-only sanity: the trait surface is shaped the way the ADR
    /// promises. If this stops compiling, the public API of the
    /// skeleton has drifted from the ADR.
    fn _api_shape_check(
        adapter: AgentAttestationAdapter,
    ) -> (Arc<dyn AgentAttestationSigner>, Arc<dyn SoulAttester>) {
        let a: Arc<dyn AgentAttestationSigner> = Arc::new(AgentAttestationAdapter {
            custody: adapter.custody.clone(),
        });
        let b: Arc<dyn SoulAttester> = Arc::new(AgentAttestationAdapter {
            custody: adapter.custody,
        });
        (a, b)
    }
}
