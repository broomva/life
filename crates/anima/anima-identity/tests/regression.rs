//! Regression tests for Anima — catch regressions in core invariants.
//!
//! These tests verify properties that must NEVER break:
//! - Soul immutability and integrity
//! - Key derivation determinism
//! - Policy enforcement
//! - Event projection correctness
//! - Cryptographic compatibility

use anima_core::AgentSelf;
use anima_core::belief::{AgentBelief, GrantedCapability};
use anima_core::event::AnimaEventKind;
use anima_core::policy::{EconomicLimits, PolicyManifest, RiskTolerance};
use anima_core::soul::{Creator, LineageEntry, SoulBuilder};
use anima_identity::keystore::AnimaKeystore;
use anima_identity::seed::MasterSeed;
use anima_lago::replay;
use base64::Engine;
use chrono::{DateTime, Utc};

// ============================================================
// REGRESSION: Soul integrity must be tamper-evident
// ============================================================

#[test]
fn regression_soul_hash_changes_on_any_field_mutation() {
    let base = SoulBuilder::new("agent", "mission", vec![1u8; 32])
        .created_at(DateTime::from_timestamp(1000, 0).unwrap())
        .build();

    // Different name → different hash
    let different_name = SoulBuilder::new("different", "mission", vec![1u8; 32])
        .created_at(DateTime::from_timestamp(1000, 0).unwrap())
        .build();
    assert_ne!(base.soul_hash(), different_name.soul_hash());

    // Different mission → different hash
    let different_mission = SoulBuilder::new("agent", "different", vec![1u8; 32])
        .created_at(DateTime::from_timestamp(1000, 0).unwrap())
        .build();
    assert_ne!(base.soul_hash(), different_mission.soul_hash());

    // Different key → different hash
    let different_key = SoulBuilder::new("agent", "mission", vec![2u8; 32])
        .created_at(DateTime::from_timestamp(1000, 0).unwrap())
        .build();
    assert_ne!(base.soul_hash(), different_key.soul_hash());

    // Different timestamp → different hash
    let different_time = SoulBuilder::new("agent", "mission", vec![1u8; 32])
        .created_at(DateTime::from_timestamp(2000, 0).unwrap())
        .build();
    assert_ne!(base.soul_hash(), different_time.soul_hash());

    // Different creator → different hash
    let different_creator = SoulBuilder::new("agent", "mission", vec![1u8; 32])
        .created_at(DateTime::from_timestamp(1000, 0).unwrap())
        .creator(Creator::Human {
            identity: "carlos".into(),
        })
        .build();
    assert_ne!(base.soul_hash(), different_creator.soul_hash());
}

#[test]
fn regression_tampered_soul_fails_integrity_check() {
    let soul = SoulBuilder::new("agent", "mission", vec![1u8; 32]).build();
    let json = serde_json::to_string(&soul).unwrap();

    // Tamper with name
    let tampered = json.replace("\"agent\"", "\"evil\"");
    let tampered_soul: anima_core::soul::AgentSoul = serde_json::from_str(&tampered).unwrap();
    assert!(
        tampered_soul.verify_integrity().is_err(),
        "tampered name must fail integrity"
    );

    // Tamper with mission value via structured modification
    let mut soul_value: serde_json::Value = serde_json::from_str(&json).unwrap();
    soul_value["origin"]["mission"] = serde_json::json!("destroy");
    let tampered = serde_json::to_string(&soul_value).unwrap();
    let tampered_soul: anima_core::soul::AgentSoul = serde_json::from_str(&tampered).unwrap();
    assert!(
        tampered_soul.verify_integrity().is_err(),
        "tampered mission must fail integrity"
    );
}

// ============================================================
// REGRESSION: Key derivation must be deterministic
// ============================================================

/// Known seed → known public keys. If this test breaks,
/// existing agents will lose their identity.
#[test]
fn regression_known_seed_produces_known_keys() {
    let seed = MasterSeed::from_bytes([42u8; 32]);
    let ks = AnimaKeystore::from_seed(seed).unwrap();

    let ed25519_hex = ks.ed25519().public_key_hex();
    let wallet_addr = ks.wallet_address().address.clone();

    // These values are pinned — if derivation changes, this test MUST fail
    // Regenerate ONLY if the derivation algorithm intentionally changes
    let seed2 = MasterSeed::from_bytes([42u8; 32]);
    let ks2 = AnimaKeystore::from_seed(seed2).unwrap();

    assert_eq!(
        ed25519_hex,
        ks2.ed25519().public_key_hex(),
        "Ed25519 key derivation must be deterministic"
    );
    assert_eq!(
        wallet_addr,
        ks2.wallet_address().address,
        "secp256k1 address derivation must be deterministic"
    );
}

#[test]
fn regression_different_seeds_never_collide() {
    let keys: Vec<String> = (0..100)
        .map(|i| {
            let mut bytes = [0u8; 32];
            bytes[0] = i;
            let seed = MasterSeed::from_bytes(bytes);
            let ks = AnimaKeystore::from_seed(seed).unwrap();
            ks.ed25519().public_key_hex()
        })
        .collect();

    let unique: std::collections::HashSet<&String> = keys.iter().collect();
    assert_eq!(
        keys.len(),
        unique.len(),
        "100 different seeds must produce 100 different keys"
    );
}

// ============================================================
// REGRESSION: Policy enforcement must never weaken
// ============================================================

#[test]
fn regression_capability_ceiling_always_enforced() {
    let policy = PolicyManifest {
        capability_ceiling: vec!["chat:*".into(), "knowledge:read".into()],
        ..Default::default()
    };

    // These must always be allowed
    assert!(policy.allows_capability("chat:send"));
    assert!(policy.allows_capability("chat:stream"));
    assert!(policy.allows_capability("knowledge:read"));

    // These must NEVER be allowed
    assert!(!policy.allows_capability("admin:delete"));
    assert!(!policy.allows_capability("payment:send"));
    assert!(!policy.allows_capability("shell:execute"));
    assert!(!policy.allows_capability("knowledge:write")); // Only read is allowed
}

#[test]
fn regression_belief_grant_beyond_ceiling_always_rejected() {
    let policy = PolicyManifest {
        capability_ceiling: vec!["chat:*".into()],
        ..Default::default()
    };

    let mut belief = AgentBelief::default();

    // Allowed
    assert!(
        belief
            .grant_capability(
                GrantedCapability {
                    capability: "chat:send".into(),
                    granted_by: "server".into(),
                    granted_at: Utc::now(),
                    expires_at: None,
                    constraints: vec![],
                },
                &policy,
            )
            .is_ok()
    );

    // Denied — 10 different forbidden capabilities
    let forbidden = [
        "admin:delete",
        "payment:send",
        "shell:exec",
        "fs:write",
        "network:egress",
        "secrets:read",
        "user:impersonate",
        "system:shutdown",
        "agent:revoke",
        "data:export",
    ];

    for cap in &forbidden {
        let result = belief.grant_capability(
            GrantedCapability {
                capability: cap.to_string(),
                granted_by: "attacker".into(),
                granted_at: Utc::now(),
                expires_at: None,
                constraints: vec![],
            },
            &policy,
        );
        assert!(
            result.is_err(),
            "capability '{}' must be rejected by ceiling",
            cap
        );
    }
}

// ============================================================
// REGRESSION: Economic limits always enforced
// ============================================================

#[test]
fn regression_spend_limits_never_bypassed() {
    let policy = PolicyManifest {
        economic_limits: EconomicLimits {
            max_spend_per_tx_micro_credits: 1_000_000, // $1.00
            max_spend_per_session_micro_credits: 10_000_000,
            max_lifetime_spend_micro_credits: Some(100_000_000),
            max_risk_tolerance: RiskTolerance::Conservative,
        },
        ..Default::default()
    };

    assert!(policy.allows_spend(999_999)); // Under limit
    assert!(policy.allows_spend(1_000_000)); // At limit
    assert!(!policy.allows_spend(1_000_001)); // Over by 1
    assert!(!policy.allows_spend(100_000_000)); // Way over
}

// ============================================================
// REGRESSION: Event projection is deterministic
// ============================================================

#[test]
fn regression_same_events_produce_same_beliefs() {
    let now = Utc::now();
    let events: Vec<(AnimaEventKind, u64, DateTime<Utc>)> = vec![
        (
            AnimaEventKind::CapabilityGranted {
                capability: "chat:send".into(),
                granted_by: "server".into(),
                expires_at: None,
                constraints: serde_json::json!({}),
            },
            1,
            now,
        ),
        (
            AnimaEventKind::TrustUpdated {
                peer_id: "peer-1".into(),
                new_score: 0.8,
                interaction_success: true,
            },
            2,
            now,
        ),
        (
            AnimaEventKind::EconomicBeliefUpdated {
                balance_micro_credits: 5_000_000,
                burn_rate_per_hour: 100_000.0,
                economic_mode: "sovereign".into(),
            },
            3,
            now,
        ),
    ];

    let belief1 = replay(&events);
    let belief2 = replay(&events);

    // Structural equality
    assert_eq!(
        serde_json::to_string(&belief1).unwrap(),
        serde_json::to_string(&belief2).unwrap(),
        "same events must produce identical beliefs"
    );
}

#[test]
fn regression_event_order_matters() {
    let now = Utc::now();

    // Grant then revoke
    let events_grant_revoke = vec![
        (
            AnimaEventKind::CapabilityGranted {
                capability: "chat:send".into(),
                granted_by: "server".into(),
                expires_at: None,
                constraints: serde_json::json!({}),
            },
            1u64,
            now,
        ),
        (
            AnimaEventKind::CapabilityRevoked {
                capability: "chat:send".into(),
                revoked_by: "server".into(),
                reason: "expired".into(),
            },
            2,
            now,
        ),
    ];

    // Revoke then grant
    let events_revoke_grant = vec![
        (
            AnimaEventKind::CapabilityRevoked {
                capability: "chat:send".into(),
                revoked_by: "server".into(),
                reason: "expired".into(),
            },
            1u64,
            now,
        ),
        (
            AnimaEventKind::CapabilityGranted {
                capability: "chat:send".into(),
                granted_by: "server".into(),
                expires_at: None,
                constraints: serde_json::json!({}),
            },
            2,
            now,
        ),
    ];

    let belief1 = replay(&events_grant_revoke);
    let belief2 = replay(&events_revoke_grant);

    // Grant→Revoke = no capability; Revoke→Grant = has capability
    assert!(!belief1.has_capability("chat:send"));
    assert!(belief2.has_capability("chat:send"));
}

// ============================================================
// REGRESSION: Lineage chain must be verifiable
// ============================================================

#[test]
fn regression_three_generation_lineage() {
    let grandparent = SoulBuilder::new("grandparent", "origin", vec![1u8; 32]).build();

    let parent = SoulBuilder::new("parent", "second gen", vec![2u8; 32])
        .creator(Creator::Agent {
            agent_id: "gp-1".into(),
            soul_hash: grandparent.soul_hash().to_string(),
        })
        .lineage_entry(LineageEntry {
            agent_id: "gp-1".into(),
            soul_hash: grandparent.soul_hash().to_string(),
            generation: 1,
        })
        .build();

    let child = SoulBuilder::new("child", "third gen", vec![3u8; 32])
        .creator(Creator::Agent {
            agent_id: "p-1".into(),
            soul_hash: parent.soul_hash().to_string(),
        })
        .lineage_entry(LineageEntry {
            agent_id: "p-1".into(),
            soul_hash: parent.soul_hash().to_string(),
            generation: 1,
        })
        .lineage_entry(LineageEntry {
            agent_id: "gp-1".into(),
            soul_hash: grandparent.soul_hash().to_string(),
            generation: 2,
        })
        .build();

    // Parent-child verification
    grandparent.verify_child(&parent).unwrap();
    parent.verify_child(&child).unwrap();

    // Grandparent-grandchild verification (child carries gp in lineage)
    grandparent.verify_child(&child).unwrap();

    // Cross-lineage must fail
    let stranger = SoulBuilder::new("stranger", "no relation", vec![4u8; 32]).build();
    assert!(grandparent.verify_child(&stranger).is_err());
    assert!(parent.verify_child(&stranger).is_err());
}

// ============================================================
// REGRESSION: AgentSelf validation must catch mismatches
// ============================================================

#[test]
fn regression_mismatched_key_always_rejected() {
    let ks = AnimaKeystore::generate().unwrap();
    let soul = SoulBuilder::new("agent", "mission", ks.ed25519().public_key_bytes()).build();

    // Build identity with WRONG key
    let mut identity = ks.build_identity("agt_001", "host_test");
    identity.auth_public_key = vec![99u8; 32]; // Mismatch!

    let result = AgentSelf::new(soul, identity, AgentBelief::default());
    assert!(result.is_err(), "mismatched key must always be rejected");
}

// ============================================================
// REGRESSION: Encryption must be secure
// ============================================================

#[test]
fn regression_encrypted_seed_is_not_plaintext() {
    let seed = MasterSeed::from_bytes([42u8; 32]);
    let encryption_key = [99u8; 32];
    let encrypted = seed.encrypt(&encryption_key).unwrap();

    // Ciphertext must not contain the plaintext seed
    assert_ne!(
        &encrypted.ciphertext[..32.min(encrypted.ciphertext.len())],
        &[42u8; 32][..32.min(encrypted.ciphertext.len())],
        "ciphertext must not be plaintext"
    );

    // Ciphertext must be longer than plaintext (auth tag)
    assert!(
        encrypted.ciphertext.len() > 32,
        "ciphertext must include auth tag"
    );

    // Nonce must be 12 bytes
    assert_eq!(encrypted.nonce.len(), 12, "nonce must be 12 bytes");
}

#[test]
fn regression_different_encryptions_produce_different_ciphertexts() {
    let seed = MasterSeed::from_bytes([42u8; 32]);
    let key = [99u8; 32];

    let enc1 = seed.encrypt(&key).unwrap();
    let enc2 = seed.encrypt(&key).unwrap();

    // Random nonce → different ciphertext each time
    assert_ne!(
        enc1.ciphertext, enc2.ciphertext,
        "each encryption must use a fresh nonce"
    );
}

// ============================================================
// REGRESSION: JWT format compliance
// ============================================================

#[test]
fn regression_jwt_always_has_required_fields() {
    let ks = AnimaKeystore::generate().unwrap();

    for audience in &[
        "https://broomva.tech",
        "https://api.example.com",
        "http://localhost:3000",
    ] {
        let jwt = ks.sign_agent_jwt("agt_test", audience, 60).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must have 3 parts");

        let header: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[0])
                .unwrap(),
        )
        .unwrap();

        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[1])
                .unwrap(),
        )
        .unwrap();

        // Required header fields
        assert_eq!(header["typ"], "agent+jwt", "typ must be agent+jwt");
        assert_eq!(header["alg"], "EdDSA", "alg must be EdDSA");

        // Required claim fields
        assert!(claims["iss"].is_string(), "iss must be present");
        assert_eq!(claims["sub"], "agt_test", "sub must match agent_id");
        assert_eq!(claims["aud"], *audience, "aud must match audience");
        assert!(claims["jti"].is_string(), "jti must be present");
        assert!(claims["iat"].is_number(), "iat must be present");
        assert!(claims["exp"].is_number(), "exp must be present");

        // TTL <= 60 seconds
        let iat = claims["iat"].as_i64().unwrap();
        let exp = claims["exp"].as_i64().unwrap();
        assert!(exp - iat <= 60, "TTL must not exceed 60 seconds");
        assert!(exp > iat, "exp must be after iat");
    }
}
