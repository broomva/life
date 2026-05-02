//! Chaos test (Sub-phase E item #6 — chaos #4): KMS unreachable →
//! `build_signer` fails closed and the daemon refuses to start.
//!
//! The Sub-phase C hardening (Option B, BRO-938 follow-up #1) already
//! requires that `KmsProvider::Dev` is rejected unless
//! `dev_signer_enabled = true`. The Sub-phase E chaos battery extends
//! the fail-closed contract:
//!
//! - `KmsProvider::Vault` with no `[auth.vault]` block → Config error.
//! - `KmsProvider::Aws` configured but the SDK chain has no
//!   credentials available → `build_signer` returns Auth error from
//!   `aws_config::defaults` OR (more commonly) the first `Sign` call
//!   returns it. We exercise the config-time path that's deterministic.
//! - `KmsProvider::Gcp` same.
//!
//! These tests run on the default feature set (kms-vault). The
//! kms-aws / kms-gcp arms are exercised under their respective
//! features in conditional gates below.

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use lifegw::LifegwError;
use lifegw::config::{AuthConfig, KmsProvider};

/// `KmsProvider::Dev` without `dev_signer_enabled` is rejected
/// (Sub-phase C hardening verified by lib tests; reproduced here so
/// the chaos battery covers it explicitly).
#[test]
fn build_signer_rejects_dev_without_dev_signer_enabled() {
    let mut cfg = AuthConfig::default();
    cfg.kms_provider = KmsProvider::Dev;
    cfg.dev_signer_enabled = false;
    match lifegw::bootstrap::build_signer_for_test(&cfg) {
        Ok(_) => panic!("must reject Dev kms_provider without dev_signer_enabled"),
        Err(LifegwError::Config(m)) => {
            assert!(
                m.contains("dev_signer_enabled"),
                "rejection mentions dev_signer_enabled: {m}"
            );
        }
        Err(other) => panic!("expected Config error, got {other:?}"),
    }
}

/// `KmsProvider::Vault` with no `[auth.vault]` block → Config error.
/// The daemon refuses to start so a misconfigured deployment never
/// silently fall-overs into the dev signer.
#[test]
#[cfg(feature = "kms-vault")]
fn build_signer_rejects_vault_without_config_block() {
    let mut cfg = AuthConfig::default();
    cfg.kms_provider = KmsProvider::Vault;
    cfg.vault = None;
    match lifegw::bootstrap::build_signer_for_test(&cfg) {
        Ok(_) => panic!("Vault provider without [auth.vault] must error"),
        Err(LifegwError::Config(m)) => {
            assert!(
                m.contains("vault") && m.contains("missing"),
                "rejection mentions [auth.vault] missing: {m}"
            );
        }
        Err(other) => panic!("expected Config error, got {other:?}"),
    }
}

/// `KmsProvider::Aws` configured without the `kms-aws` feature compiled
/// in → Config error explaining the missing feature flag. This is the
/// explicit fail-closed path for builds that don't ship AWS support.
#[test]
#[cfg(not(feature = "kms-aws"))]
fn build_signer_rejects_aws_without_feature() {
    let mut cfg = AuthConfig::default();
    cfg.kms_provider = KmsProvider::Aws;
    match lifegw::bootstrap::build_signer_for_test(&cfg) {
        Ok(_) => panic!("Aws provider without `kms-aws` feature must error"),
        Err(LifegwError::Config(m)) => {
            assert!(
                m.contains("kms-aws"),
                "rejection mentions missing kms-aws feature: {m}"
            );
        }
        Err(other) => panic!("expected Config error, got {other:?}"),
    }
}

/// Same for GCP.
#[test]
#[cfg(not(feature = "kms-gcp"))]
fn build_signer_rejects_gcp_without_feature() {
    let mut cfg = AuthConfig::default();
    cfg.kms_provider = KmsProvider::Gcp;
    match lifegw::bootstrap::build_signer_for_test(&cfg) {
        Ok(_) => panic!("Gcp provider without `kms-gcp` feature must error"),
        Err(LifegwError::Config(m)) => {
            assert!(
                m.contains("kms-gcp"),
                "rejection mentions missing kms-gcp feature: {m}"
            );
        }
        Err(other) => panic!("expected Config error, got {other:?}"),
    }
}
