//! Conformance harness for lifed's substrate-token verification.
//!
//! Spec C₂ §15.5: every substrate (arcan, lago, haima, anima, soma) must
//! agree on the verification rules for Tier-3 JWS bearer tokens minted by
//! lifed. This crate exposes:
//!
//! - `SubstrateUnderTest` — the trait each substrate's CI lane implements
//!   (or wraps via `reference_verify` for the reference path).
//! - `VerificationCase` — one bearer + expected outcome.
//! - `run_battery` — drives a vec of cases against a `SubstrateUnderTest`
//!   and asserts the accept/reject outcome matches.
//! - `reference_verify` — a generic ES256 verifier substrates can plug in
//!   if they don't yet have a hand-rolled implementation.

#![deny(unsafe_code)]

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConformanceError {
    #[error("setup: {0}")]
    Setup(String),
    #[error("expectation '{0}' violated: {1}")]
    Failed(&'static str, String),
}

pub struct VerificationCase {
    pub name: &'static str,
    pub jws: String,
    pub expected_aud: &'static str,
    pub should_pass: bool,
}

#[async_trait]
pub trait SubstrateUnderTest: Send + Sync {
    fn audience(&self) -> &'static str;
    async fn verify(
        &self,
        jws: &str,
        audience: &str,
        pubkey_pem: &str,
    ) -> Result<(), tonic::Status>;
}

pub async fn run_battery(
    sut: &dyn SubstrateUnderTest,
    pubkey_pem: &str,
    cases: &[VerificationCase],
) -> Result<(), ConformanceError> {
    for c in cases {
        let res = sut.verify(&c.jws, c.expected_aud, pubkey_pem).await;
        match (res, c.should_pass) {
            (Ok(()), true) => continue,
            (Err(_), false) => continue,
            (Ok(()), false) => {
                return Err(ConformanceError::Failed(
                    c.name,
                    format!("expected reject, got accept: {}", c.jws),
                ));
            }
            (Err(e), true) => {
                return Err(ConformanceError::Failed(
                    c.name,
                    format!("expected accept, got reject: {e}"),
                ));
            }
        }
    }
    Ok(())
}

/// Convenience generic verifier that uses jsonwebtoken directly. Real
/// substrates may have their own; this is the reference impl + the
/// substrate-side check that any conforming implementation must match.
pub fn reference_verify(jws: &str, audience: &str, pubkey_pem: &str) -> Result<(), tonic::Status> {
    let key = DecodingKey::from_ec_pem(pubkey_pem.as_bytes())
        .map_err(|e| tonic::Status::internal(format!("decode pem: {e}")))?;
    let mut v = Validation::new(Algorithm::ES256);
    v.set_audience(&[audience]);
    v.set_issuer(&["lifed"]);
    decode::<serde_json::Value>(jws, &key, &v)
        .map(|_| ())
        .map_err(|e| tonic::Status::unauthenticated(format!("verify: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubSubstrate;

    #[async_trait]
    impl SubstrateUnderTest for StubSubstrate {
        fn audience(&self) -> &'static str {
            "stub"
        }
        async fn verify(
            &self,
            _jws: &str,
            _audience: &str,
            _pubkey_pem: &str,
        ) -> Result<(), tonic::Status> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_battery_passes_for_any_substrate() {
        let sut = StubSubstrate;
        run_battery(&sut, "irrelevant", &[]).await.expect("noop");
    }
}
