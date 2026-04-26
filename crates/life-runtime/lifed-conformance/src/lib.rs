//! Conformance harness for lifed's substrate-token verification.
//!
//! Each substrate's CI lane runs this harness as a smoke test to verify
//! that:
//! - lifed's published JWKS at `/run/life/lifed-jwks.json` parses cleanly.
//! - The substrate's verifier accepts a freshly minted Tier-3 token.
//! - The verifier REJECTS tokens with the wrong `aud`.
//! - The verifier REJECTS expired tokens.
//!
//! Sub-phase A places only the trait scaffold; sub-phase B's task B17
//! populates the test bodies.

#![deny(unsafe_code)]

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConformanceError {
    #[error("setup: {0}")]
    Setup(String),
    #[error("expectation '{0}' violated: {1}")]
    Failed(&'static str, String),
}

#[async_trait]
pub trait SubstrateUnderTest: Send + Sync {
    /// Substrate name — `arcan`, `lago`, `haima`, `anima`, `soma`.
    fn audience(&self) -> &'static str;
    /// Verify a Tier-3 JWS bearer token. Returns Ok(()) on success.
    async fn verify(&self, jws: &str) -> Result<(), tonic::Status>;
}

/// Sub-phase A no-op — B17 fills in the real conformance battery.
pub async fn run_battery(_sut: &dyn SubstrateUnderTest) -> Result<(), ConformanceError> {
    Ok(())
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
        async fn verify(&self, _jws: &str) -> Result<(), tonic::Status> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn battery_is_a_noop_in_sub_phase_a() {
        let sut = StubSubstrate;
        run_battery(&sut).await.expect("noop battery passes");
    }
}
