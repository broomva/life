//! Integration tests for KYA (Know Your Agent) identity flow.
//!
//! Tests the complete KYA lifecycle:
//! 1. Generate keystore (seed → dual keypair)
//! 2. Derive DID from Ed25519 public key
//! 3. Build AgentIdentity with DID
//! 4. Create AgentSelf
//! 5. Generate identity document
//! 6. Verify DID resolution roundtrip
//! 7. Attestation management
//! 8. Trust score integration
//! 9. Lago event persistence for identity events

use anima_core::agent_self::AgentSelf;
use anima_core::belief::AgentBelief;
use anima_core::event::AnimaEventKind;
use anima_core::identity_document::{AgentType, TrustTier};
use anima_core::soul::SoulBuilder;
use anima_identity::did::{
    generate_did_key, resolve_did_key, verification_method_id, verify_did_key,
};
use anima_identity::keystore::AnimaKeystore;
use anima_identity::seed::MasterSeed;
use anima_lago::replay;
use chrono::Utc;

/// Full KYA lifecycle — from seed to identity document.
#[test]
fn kya_full_lifecycle() {
    // Step 1: Generate keystore
    let keystore = AnimaKeystore::generate().unwrap();

    // Step 2: Derive DID
    let pubkey_bytes: [u8; 32] = keystore
        .ed25519()
        .public_key_bytes()
        .as_slice()
        .try_into()
        .unwrap();
    let did = generate_did_key(&pubkey_bytes);
    assert!(did.starts_with("did:key:z6Mk"));

    // Step 3: Verify DID resolves back to the same key
    let resolved = resolve_did_key(&did).unwrap();
    assert_eq!(resolved, pubkey_bytes);

    // Step 4: Build AgentIdentity (keystore already sets DID)
    let identity = keystore.build_identity("agt_kya_test_001", "host_arcan");
    assert_eq!(identity.did.as_ref().unwrap(), &did);

    // Step 5: Create soul and AgentSelf
    let soul = SoulBuilder::new(
        "kya-test-agent",
        "Test KYA identity lifecycle",
        keystore.ed25519().public_key_bytes(),
    )
    .build();

    let agent = AgentSelf::new(soul, identity, AgentBelief::default()).unwrap();

    // Step 6: Generate identity document
    let doc = agent
        .identity_document(AgentType::Autonomous, Some(0.85))
        .unwrap();

    assert_eq!(doc.did, did);
    assert_eq!(doc.agent_type, AgentType::Autonomous);
    assert_eq!(doc.trust_score, Some(0.85));
    assert_eq!(doc.trust_tier, Some(TrustTier::Trusted));
    assert_eq!(doc.name, "kya-test-agent");
    assert_eq!(doc.mission, "Test KYA identity lifecycle");
    assert_eq!(doc.verification_methods.len(), 1);

    // Verify the verification method
    let vm = &doc.verification_methods[0];
    assert_eq!(vm.method_type, "Ed25519VerificationKey2020");
    assert!(vm.id.ends_with("#key-1"));
    assert!(vm.public_key_multibase.starts_with('z'));

    // Step 7: Serialize and deserialize the document
    let json = serde_json::to_string_pretty(&doc).unwrap();
    let recovered: anima_core::identity_document::AgentIdentityDocument =
        serde_json::from_str(&json).unwrap();
    assert_eq!(doc, recovered);

    println!("\n--- KYA Identity Document ---");
    println!("{json}");
    println!("\n[OK] KYA full lifecycle passed");
}

/// DID derivation is deterministic across keystore instances.
#[test]
fn kya_did_deterministic_from_seed() {
    let seed_bytes = [42u8; 32];

    let ks1 = AnimaKeystore::from_seed(MasterSeed::from_bytes(seed_bytes)).unwrap();
    let ks2 = AnimaKeystore::from_seed(MasterSeed::from_bytes(seed_bytes)).unwrap();

    let did1 = ks1.ed25519().did_key();
    let did2 = ks2.ed25519().did_key();
    assert_eq!(did1, did2, "same seed must produce same DID");

    // Also verify via the standalone function
    let pubkey: [u8; 32] = ks1
        .ed25519()
        .public_key_bytes()
        .as_slice()
        .try_into()
        .unwrap();
    let did3 = generate_did_key(&pubkey);
    assert_eq!(
        did1, did3,
        "Ed25519Identity::did_key and generate_did_key must agree"
    );
}

/// Identity document with delegated agent type and controller DID.
#[test]
fn kya_delegated_agent_with_controller() {
    let human_keystore = AnimaKeystore::generate().unwrap();
    let agent_keystore = AnimaKeystore::generate().unwrap();

    let human_pubkey: [u8; 32] = human_keystore
        .ed25519()
        .public_key_bytes()
        .as_slice()
        .try_into()
        .unwrap();
    let human_did = generate_did_key(&human_pubkey);

    let soul = SoulBuilder::new(
        "delegated-agent",
        "Acts on behalf of a human",
        agent_keystore.ed25519().public_key_bytes(),
    )
    .build();

    let identity = agent_keystore.build_identity("agt_delegated_001", "host_arcan");
    let agent = AgentSelf::new(soul, identity, AgentBelief::default()).unwrap();

    let mut doc = agent
        .identity_document(AgentType::Delegated, Some(0.5))
        .unwrap();

    doc.controller_did = Some(human_did.clone());

    assert_eq!(doc.agent_type, AgentType::Delegated);
    assert_eq!(doc.controller_did, Some(human_did));
    assert_eq!(doc.trust_tier, Some(TrustTier::Provisional));
}

/// DID verification — verify that a DID matches a known public key.
#[test]
fn kya_did_verification() {
    let ks = AnimaKeystore::generate().unwrap();
    let pubkey: [u8; 32] = ks
        .ed25519()
        .public_key_bytes()
        .as_slice()
        .try_into()
        .unwrap();

    let did = generate_did_key(&pubkey);

    // Correct key verifies
    assert!(verify_did_key(&did, &pubkey));

    // Wrong key fails
    let wrong_key = [99u8; 32];
    assert!(!verify_did_key(&did, &wrong_key));
}

/// Verification method ID follows did:key spec.
#[test]
fn kya_verification_method_id() {
    let ks = AnimaKeystore::generate().unwrap();
    let pubkey: [u8; 32] = ks
        .ed25519()
        .public_key_bytes()
        .as_slice()
        .try_into()
        .unwrap();

    let did = generate_did_key(&pubkey);
    let vm_id = verification_method_id(&did);

    assert!(vm_id.starts_with("did:key:z"));
    assert!(vm_id.ends_with("#key-1"));
    assert_eq!(vm_id, format!("{did}#key-1"));
}

/// Identity events round-trip through Lago projection.
#[test]
fn kya_identity_events_in_lago() {
    let now = Utc::now();

    let events = vec![
        // Standard capability events
        (
            AnimaEventKind::CapabilityGranted {
                capability: "chat:send".into(),
                granted_by: "broomva.tech".into(),
                expires_at: None,
                constraints: serde_json::json!({}),
            },
            1u64,
            now,
        ),
        // KYA identity events (don't modify beliefs but are persisted)
        (
            AnimaEventKind::IdentityCreated {
                agent_id: "agt_001".into(),
                host_id: "host_arcan".into(),
                auth_public_key_hex: "abcd1234".into(),
                wallet_address: "0xtest".into(),
                did: Some("did:key:z6MkTest".into()),
                seed_blob_ref: None,
            },
            2,
            now,
        ),
        (
            AnimaEventKind::IdentityAttested {
                issuer_did: "did:key:z6MkIssuer".into(),
                claim: "safety-audit-passed".into(),
                evidence: "lago:event:456".into(),
                expires_at: None,
            },
            3,
            now,
        ),
        (
            AnimaEventKind::IdentityVerified {
                verifier_did: "did:key:z6MkVerifier".into(),
                method: "did-auth-challenge".into(),
                verified: true,
            },
            4,
            now,
        ),
    ];

    // Replay should succeed without panicking
    let beliefs = replay(&events);

    // The capability grant should be reflected
    assert!(beliefs.has_capability("chat:send"));
    // Identity events don't modify beliefs
    assert_eq!(beliefs.capabilities.len(), 1);
    // But the last event seq should advance
    assert_eq!(beliefs.last_event_seq, 4);
}

/// Event type strings for new KYA events.
#[test]
fn kya_event_type_strings() {
    let attested = AnimaEventKind::IdentityAttested {
        issuer_did: "did:key:z6MkIssuer".into(),
        claim: "verified".into(),
        evidence: "proof".into(),
        expires_at: None,
    };
    assert_eq!(attested.event_type(), "anima.identity_attested");

    let verified = AnimaEventKind::IdentityVerified {
        verifier_did: "did:key:z6MkVerifier".into(),
        method: "challenge-response".into(),
        verified: true,
    };
    assert_eq!(verified.event_type(), "anima.identity_verified");
}

/// KYA events serialize and deserialize through custom data format.
#[test]
fn kya_events_roundtrip_through_custom() {
    let original = AnimaEventKind::IdentityAttested {
        issuer_did: "did:key:z6MkIssuer".into(),
        claim: "safety-audit-passed".into(),
        evidence: "lago:event:789".into(),
        expires_at: Some(Utc::now() + chrono::Duration::days(365)),
    };

    let event_type = original.event_type();
    let data = original.to_custom_data();

    let parsed = AnimaEventKind::from_custom(&event_type, &data);
    assert_eq!(parsed, Some(original));
}

/// Trust tiers at boundary values.
#[test]
fn kya_trust_tier_boundaries() {
    // Just below each threshold
    assert_eq!(TrustTier::from_score(0.39), TrustTier::Unverified);
    assert_eq!(TrustTier::from_score(0.69), TrustTier::Provisional);
    assert_eq!(TrustTier::from_score(0.89), TrustTier::Trusted);

    // Exactly at each threshold
    assert_eq!(TrustTier::from_score(0.4), TrustTier::Provisional);
    assert_eq!(TrustTier::from_score(0.7), TrustTier::Trusted);
    assert_eq!(TrustTier::from_score(0.9), TrustTier::Certified);
}

/// Identity document with capabilities from beliefs.
#[test]
fn kya_document_reflects_active_capabilities() {
    let ks = AnimaKeystore::generate().unwrap();
    let soul = SoulBuilder::new(
        "cap-test",
        "Test capabilities in KYA doc",
        ks.ed25519().public_key_bytes(),
    )
    .build();

    let identity = ks.build_identity("agt_cap_test", "host_test");
    let now = Utc::now();

    // Build beliefs with events
    let events = vec![
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
            AnimaEventKind::CapabilityGranted {
                capability: "knowledge:read".into(),
                granted_by: "server".into(),
                expires_at: None,
                constraints: serde_json::json!({}),
            },
            2,
            now,
        ),
        (
            AnimaEventKind::CapabilityGranted {
                capability: "tool:execute".into(),
                granted_by: "server".into(),
                // Already expired
                expires_at: Some(now - chrono::Duration::hours(1)),
                constraints: serde_json::json!({}),
            },
            3,
            now,
        ),
    ];

    let beliefs = replay(&events);
    let agent = AgentSelf::from_parts_unchecked(soul, identity, beliefs);

    let doc = agent.identity_document(AgentType::Hosted, None).unwrap();

    // Only active capabilities should appear (not expired tool:execute)
    assert_eq!(doc.capabilities.len(), 2);
    assert!(doc.has_capability("chat:send"));
    assert!(doc.has_capability("knowledge:read"));
    assert!(!doc.has_capability("tool:execute"));

    // No trust score was provided
    assert_eq!(doc.trust_score, None);
    assert_eq!(doc.trust_tier, None);
}
