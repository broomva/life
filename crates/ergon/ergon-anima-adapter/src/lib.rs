//! Anima-backed implementation of ergon attestation traits.
//!
//! See `docs/architecture/adr/2026-05-22-anima-signing-surface-for-ergon-attestation.md`
//! (BRO-1226) for the design rationale.
//!
//! Implemented 2026-06-10 (harness Phase-2 gap closure): both
//! `sign_step_receipt` and the two `SoulAttester` boundaries produce
//! real compact JWSs through the agent's [`AnimaCustody`] key. ADR
//! resolutions: Open §2 (journal abstraction) — the adapter signs and
//! emits the JWS on the trace span; durable journal emission stays at
//! the hook/host layer, which owns journal handles. Open §3
//! (canonical JSON) — receipt claims are built as `serde_json` maps
//! (BTreeMap-backed → key-sorted serialization) with flat ASCII keys,
//! which is canonical for this shape; full RFC 8785 lands if receipts
//! ever grow nested/non-ASCII content.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
    async fn sign_step_receipt(&self, receipt: &Value) -> std::result::Result<String, String>;
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

    /// Build + sign a session-boundary receipt. The claims map is
    /// flat, ASCII-keyed and `serde_json`-map-backed (BTreeMap →
    /// key-sorted serialization), which is canonical for this shape
    /// (ADR Open §3).
    fn sign_session_boundary(
        &self,
        kind: &str,
        session_id: &SessionId,
        workflow_name: &str,
        ok: Option<bool>,
    ) -> std::result::Result<String, String> {
        let mut claims = serde_json::json!({
            "kind": kind,
            "session_id": session_id.as_str(),
            "workflow": workflow_name,
            "agent_did": self.custody.user_did(),
            "iat": unix_now_secs(),
        });
        if let Some(ok) = ok {
            claims["ok"] = Value::Bool(ok);
        }
        self.custody
            .sign_jws(&claims)
            .map_err(|e| format!("anima sign_jws failed for {kind}: {e}"))
    }
}

/// Seconds since the Unix epoch — the `iat` claim on every receipt.
fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[async_trait]
impl AgentAttestationSigner for AgentAttestationAdapter {
    async fn sign_step_receipt(&self, receipt: &Value) -> std::result::Result<String, String> {
        self.custody.sign_jws(receipt).map_err(|e| {
            tracing::warn!(
                did = self.custody.user_did(),
                error = %e,
                "anima step-receipt signing failed"
            );
            format!("anima sign_jws failed for step receipt: {e}")
        })
    }
}

#[async_trait]
impl SoulAttester for AgentAttestationAdapter {
    async fn sign_session_start(
        &self,
        session_id: &SessionId,
        workflow_name: &str,
    ) -> std::result::Result<(), String> {
        let jws =
            self.sign_session_boundary("ergon.session_start", session_id, workflow_name, None)?;
        tracing::info!(
            session = %session_id.as_str(),
            workflow = workflow_name,
            did = self.custody.user_did(),
            jws = %jws,
            "session start attested"
        );
        Ok(())
    }

    async fn sign_session_end(
        &self,
        session_id: &SessionId,
        workflow_name: &str,
        ok: bool,
    ) -> std::result::Result<(), String> {
        let jws =
            self.sign_session_boundary("ergon.session_end", session_id, workflow_name, Some(ok))?;
        tracing::info!(
            session = %session_id.as_str(),
            workflow = workflow_name,
            did = self.custody.user_did(),
            ok,
            jws = %jws,
            "session end attested"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use anima_identity::InProcessAnima;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use super::*;

    /// Compile-only sanity: the trait surface is shaped the way the ADR
    /// promises. If this stops compiling, the public API has drifted
    /// from the ADR.
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

    fn adapter() -> AgentAttestationAdapter {
        AgentAttestationAdapter::new(InProcessAnima::generate_dev().expect("dev custody"))
    }

    fn decode_claims(jws: &str) -> Value {
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(parts.len(), 3, "compact JWS has three segments");
        let body = URL_SAFE_NO_PAD.decode(parts[1]).expect("base64url body");
        serde_json::from_slice(&body).expect("claims JSON")
    }

    #[tokio::test]
    async fn session_start_signs_real_jws_with_expected_claims() {
        let adapter = adapter();
        let did = adapter.agent_did().to_owned();
        let sid = SessionId::from_string("sid-attest");
        adapter
            .sign_session_start(&sid, "greeter")
            .await
            .expect("attest ok");
        // Sign again via the internal helper to inspect the claims the
        // boundary produces (the trait surface logs the JWS).
        let jws = adapter
            .sign_session_boundary("ergon.session_start", &sid, "greeter", None)
            .expect("sign");
        let claims = decode_claims(&jws);
        assert_eq!(claims["kind"], "ergon.session_start");
        assert_eq!(claims["session_id"], "sid-attest");
        assert_eq!(claims["workflow"], "greeter");
        assert_eq!(claims["agent_did"], did);
        assert!(claims["iat"].as_u64().unwrap_or(0) > 1_700_000_000);
    }

    #[tokio::test]
    async fn session_end_carries_ok_flag() {
        let adapter = adapter();
        let sid = SessionId::from_string("sid-attest-end");
        adapter
            .sign_session_end(&sid, "greeter", false)
            .await
            .expect("attest ok");
        let jws = adapter
            .sign_session_boundary("ergon.session_end", &sid, "greeter", Some(false))
            .expect("sign");
        let claims = decode_claims(&jws);
        assert_eq!(claims["kind"], "ergon.session_end");
        assert_eq!(claims["ok"], false);
    }

    /// Guard for the module-doc canonicalization claim: the receipt
    /// claims serialize key-sorted, which holds only while the
    /// workspace builds `serde_json` WITHOUT `preserve_order`. If a
    /// future dependency unifies that feature on, this fails loudly
    /// and the adapter needs a real canonical-JSON serializer
    /// (RFC 8785).
    #[test]
    fn serde_json_maps_serialize_key_sorted() {
        let v = serde_json::json!({"zeta": 1, "alpha": 2, "mid": 3});
        assert_eq!(v.to_string(), r#"{"alpha":2,"mid":3,"zeta":1}"#);
    }

    #[tokio::test]
    async fn step_receipt_signs_arbitrary_claims() {
        let adapter = adapter();
        let receipt = serde_json::json!({"step": 3, "tool": "fs.read"});
        let jws = adapter
            .sign_step_receipt(&receipt)
            .await
            .expect("receipt signed");
        let claims = decode_claims(&jws);
        assert_eq!(claims["step"], 3);
        assert_eq!(claims["tool"], "fs.read");
    }
}
