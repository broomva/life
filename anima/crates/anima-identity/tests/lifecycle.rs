//! Integration test: Full agent lifecycle — from seed to soul to authenticated JWT.
//!
//! This test demonstrates the complete Anima flow as it would happen
//! in production when a new agent is born:
//!
//! 1. Generate a master seed (the root of all identity)
//! 2. Derive dual keypairs (Ed25519 + secp256k1)
//! 3. Create the soul (immutable origin, values, lineage)
//! 4. Build the composite AgentSelf
//! 5. Persist as genesis event (Lago format)
//! 6. Reconstruct from event replay
//! 7. Sign an Agent Auth Protocol JWT
//! 8. Evolve beliefs (capabilities, trust, economics)
//! 9. Spawn a child agent with lineage
//! 10. Encrypt and recover the identity

use anima_core::AgentSelf;
use anima_core::belief::{AgentBelief, GrantedCapability};
use anima_core::event::AnimaEventKind;
use anima_core::identity::LifecycleState;
use anima_core::policy::{
    CommunicationPolicy, ConstraintSeverity, EconomicLimits, PolicyManifest, RiskTolerance,
    SafetyConstraint,
};
use anima_core::soul::{Creator, LineageEntry, SoulBuilder};
use anima_identity::keystore::AnimaKeystore;
use anima_lago::{create_genesis_event, fold, reconstruct_soul, replay};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;

/// The complete birth of an agent — from nothing to a fully authenticated self.
#[test]
fn agent_genesis_full_lifecycle() {
    // ============================================================
    // STEP 1: Generate the master seed
    // ============================================================
    let keystore = AnimaKeystore::generate().unwrap();

    println!("\n--- AGENT GENESIS ---");
    println!(
        "Ed25519 public key: {}",
        keystore.ed25519().public_key_hex()
    );
    println!("Wallet address:     {}", keystore.wallet_address().address);
    println!("DID:                {}", keystore.ed25519().did_key());

    // ============================================================
    // STEP 2: Define the soul
    // ============================================================
    let policy = PolicyManifest {
        safety_constraints: vec![
            SafetyConstraint {
                id: "no-impersonation".into(),
                description: "Must never impersonate a human or another agent".into(),
                severity: ConstraintSeverity::Critical,
            },
            SafetyConstraint {
                id: "no-unaudited-transfers".into(),
                description: "Must never move funds without an audit trail".into(),
                severity: ConstraintSeverity::Critical,
            },
            SafetyConstraint {
                id: "no-data-exfiltration".into(),
                description: "Must never send private data to unauthorized endpoints".into(),
                severity: ConstraintSeverity::High,
            },
        ],
        capability_ceiling: vec![
            "chat:*".into(),
            "knowledge:*".into(),
            "tool:execute".into(),
            "payment:*".into(),
        ],
        economic_limits: EconomicLimits {
            max_spend_per_tx_micro_credits: 5_000_000,
            max_spend_per_session_micro_credits: 50_000_000,
            max_lifetime_spend_micro_credits: Some(1_000_000_000),
            max_risk_tolerance: RiskTolerance::Moderate,
        },
        communication_policy: CommunicationPolicy {
            allowed_peers: vec![],
            disclosure_restrictions: vec!["private_keys".into(), "seed_material".into()],
            allow_unsolicited_contact: true,
        },
    };

    let soul = SoulBuilder::new(
        "arcan-prime",
        "Serve as the primary runtime agent for the Life Agent OS",
        keystore.ed25519().public_key_bytes(),
    )
    .creator(Creator::Human {
        identity: "carlos@broomva.tech".into(),
    })
    .values(policy)
    .build();

    println!("\n--- SOUL ---");
    println!("Name:     {}", soul.name());
    println!("Mission:  {}", soul.mission());
    println!("Hash:     {}", soul.soul_hash());
    println!("Audit:    {}", soul.audit_summary());

    soul.verify_integrity()
        .expect("soul integrity check must pass");

    // ============================================================
    // STEP 3: Build the composite AgentSelf
    // ============================================================
    let identity = keystore.build_identity("agt_arcan_prime_001", "host_arcan_v1");
    assert_eq!(identity.lifecycle, LifecycleState::Active);

    let agent = AgentSelf::new(soul.clone(), identity, AgentBelief::default()).unwrap();

    println!("\n--- AGENT SELF ---");
    println!("ID:       {}", agent.agent_id());
    println!("Active:   {}", agent.is_active());
    println!("Summary:  {}", agent.audit_summary());

    agent.validate().expect("agent self must be valid");

    // ============================================================
    // STEP 4: Persist as genesis event + reconstruct
    // ============================================================
    let genesis_event = create_genesis_event(&soul).unwrap();
    let recovered_soul = reconstruct_soul(&genesis_event).unwrap();
    assert_eq!(soul, recovered_soul);
    println!("\n[OK] Genesis event roundtrip verified");

    // ============================================================
    // STEP 5: Sign an Agent Auth Protocol JWT
    // ============================================================
    let jwt = keystore
        .sign_agent_jwt("agt_arcan_prime_001", "https://broomva.tech", 60)
        .unwrap();

    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);

    let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
    let claims_bytes = URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
    let header: serde_json::Value = serde_json::from_slice(&header_bytes).unwrap();
    let claims: serde_json::Value = serde_json::from_slice(&claims_bytes).unwrap();

    println!("\n--- AGENT AUTH JWT ---");
    println!("Header: {}", serde_json::to_string_pretty(&header).unwrap());
    println!("Claims: {}", serde_json::to_string_pretty(&claims).unwrap());

    assert_eq!(header["typ"], "agent+jwt");
    assert_eq!(header["alg"], "EdDSA");
    assert_eq!(claims["sub"], "agt_arcan_prime_001");
    assert_eq!(claims["aud"], "https://broomva.tech");

    // Verify signature
    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let sig_bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
    let sig = ed25519_dalek::Signature::from_bytes(sig_bytes.as_slice().try_into().unwrap());
    let vk = ed25519_dalek::VerifyingKey::from_bytes(
        keystore
            .ed25519()
            .public_key_bytes()
            .as_slice()
            .try_into()
            .unwrap(),
    )
    .unwrap();
    vk.verify_strict(signing_input.as_bytes(), &sig)
        .expect("JWT signature must verify");

    println!("\n[OK] JWT signature verified with Ed25519 public key");

    // ============================================================
    // STEP 6: Evolve beliefs via event replay
    // ============================================================
    let now = Utc::now();
    let events = vec![
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
        (
            AnimaEventKind::CapabilityGranted {
                capability: "knowledge:read".into(),
                granted_by: "broomva.tech".into(),
                expires_at: None,
                constraints: serde_json::json!({}),
            },
            2,
            now,
        ),
        (
            AnimaEventKind::CapabilityGranted {
                capability: "payment:send".into(),
                granted_by: "haima-gateway".into(),
                expires_at: None,
                constraints: serde_json::json!({"amount": {"maximum": 1000000}}),
            },
            3,
            now,
        ),
        (
            AnimaEventKind::TrustUpdated {
                peer_id: "lago-primary".into(),
                new_score: 0.9,
                interaction_success: true,
            },
            4,
            now,
        ),
        (
            AnimaEventKind::TrustUpdated {
                peer_id: "autonomic-controller".into(),
                new_score: 0.85,
                interaction_success: true,
            },
            5,
            now,
        ),
        (
            AnimaEventKind::EconomicBeliefUpdated {
                balance_micro_credits: 25_000_000,
                burn_rate_per_hour: 500_000.0,
                economic_mode: "sovereign".into(),
            },
            6,
            now,
        ),
    ];

    let beliefs = replay(&events);

    println!("\n--- BELIEFS (after 6 events) ---");
    println!("Capabilities:  {}", beliefs.capabilities.len());
    for cap in &beliefs.capabilities {
        println!("  - {} (from {})", cap.capability, cap.granted_by);
    }
    println!("Trust peers:   {}", beliefs.trust_scores.len());
    for (peer, score) in &beliefs.trust_scores {
        println!("  - {}: {:.2}", peer, score.score);
    }
    println!(
        "Balance:       ${:.2}",
        beliefs.economic_belief.balance_micro_credits as f64 / 1_000_000.0
    );
    println!(
        "Burn rate:     ${:.2}/hr",
        beliefs.economic_belief.burn_rate_per_hour / 1_000_000.0
    );
    println!(
        "Hours left:    {:.1}",
        beliefs
            .economic_belief
            .hours_until_exhaustion
            .unwrap_or(0.0)
    );

    beliefs
        .validate_against_policy(soul.values())
        .expect("beliefs must comply with soul policy");

    // Ceiling enforcement
    let mut test_beliefs = beliefs.clone();
    let bad_grant = GrantedCapability {
        capability: "admin:delete".into(),
        granted_by: "evil-server".into(),
        granted_at: now,
        expires_at: None,
        constraints: vec![],
    };
    assert!(
        test_beliefs
            .grant_capability(bad_grant, soul.values())
            .is_err(),
        "admin:delete must be rejected by capability ceiling"
    );
    println!("\n[OK] Capability ceiling enforced: admin:delete rejected");

    // ============================================================
    // STEP 7: Spawn a child agent with lineage
    // ============================================================
    let child_keystore = AnimaKeystore::generate().unwrap();

    let child_soul = SoulBuilder::new(
        "arcan-scout-001",
        "Autonomous research agent spawned by arcan-prime",
        child_keystore.ed25519().public_key_bytes(),
    )
    .creator(Creator::Agent {
        agent_id: "agt_arcan_prime_001".into(),
        soul_hash: soul.soul_hash().to_string(),
    })
    .lineage_entry(LineageEntry {
        agent_id: "agt_arcan_prime_001".into(),
        soul_hash: soul.soul_hash().to_string(),
        generation: 1,
    })
    .build();

    soul.verify_child(&child_soul)
        .expect("parent must verify child lineage");

    println!("\n--- CHILD AGENT ---");
    println!("Name:     {}", child_soul.name());
    println!("Parent:   {} (verified)", soul.name());

    // ============================================================
    // STEP 8: Encrypt and recover
    // ============================================================
    let encryption_key = [42u8; 32];
    let encrypted = keystore.encrypt_seed(&encryption_key).unwrap();
    let recovered = AnimaKeystore::from_encrypted(&encrypted, &encryption_key).unwrap();

    assert_eq!(
        recovered.ed25519().public_key_bytes(),
        keystore.ed25519().public_key_bytes(),
    );
    assert_eq!(
        recovered.wallet_address().address,
        keystore.wallet_address().address,
    );

    assert!(
        AnimaKeystore::from_encrypted(&encrypted, &[99u8; 32]).is_err(),
        "wrong key must fail"
    );

    println!("\n[OK] Identity encrypted, recovered, wrong-key rejected");
    println!("\n=== FULL LIFECYCLE: ALL 8 STEPS PASSED ===\n");
}

/// Snapshot recovery: events → snapshot → recovery + continued evolution
#[test]
fn belief_snapshot_and_recovery() {
    let now = Utc::now();
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
            AnimaEventKind::TrustUpdated {
                peer_id: "peer-1".into(),
                new_score: 0.9,
                interaction_success: true,
            },
            2,
            now,
        ),
    ];

    let original = replay(&events);

    let snapshot_json = serde_json::to_value(&original).unwrap();
    let snapshot_hash = blake3::hash(serde_json::to_string(&original).unwrap().as_bytes())
        .to_hex()
        .to_string();

    let recovery_events = vec![(
        AnimaEventKind::BeliefSnapshot {
            belief: snapshot_json,
            snapshot_hash,
        },
        100,
        now,
    )];

    let recovered = replay(&recovery_events);
    assert_eq!(recovered.capabilities.len(), original.capabilities.len());
    assert_eq!(recovered.trust_scores.len(), original.trust_scores.len());

    // Continue evolving after snapshot
    let mut continued = recovered;
    fold(
        &mut continued,
        &AnimaEventKind::CapabilityGranted {
            capability: "knowledge:read".into(),
            granted_by: "server".into(),
            expires_at: None,
            constraints: serde_json::json!({}),
        },
        101,
        now,
    );
    assert_eq!(continued.capabilities.len(), 2);
}

/// Haima wallet compatibility
#[test]
fn wallet_is_haima_compatible() {
    let keystore = AnimaKeystore::generate().unwrap();

    let addr = &keystore.wallet_address().address;
    assert!(addr.starts_with("0x"));
    assert_eq!(addr.len(), 42);
    assert_eq!(keystore.wallet_address().chain.0, "eip155:8453");
    assert_eq!(keystore.secp256k1_key_bytes().len(), 32);
}
