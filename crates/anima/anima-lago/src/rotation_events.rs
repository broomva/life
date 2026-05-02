//! Spec D D-Sub-E — rotation/revocation event helpers for the Lago
//! bridge.
//!
//! `AnimaCustody::rotate()` produces a [`DidRotationEvent`] but does
//! NOT itself touch the journal — that boundary is here. The trait
//! is pure (no I/O) so backends can be tested in isolation; the
//! journal-write side lives in this crate so the dependency direction
//! stays "anima-identity → anima-lago" (lago drags in redb /
//! tokio-blocking, anima-identity stays slim).
//!
//! The shape mirrors `genesis::create_genesis_event` — these helpers
//! produce `AnimaEventKind` variants ready to wrap in an
//! `EventEnvelope` and append to Lago. The actual append is done by
//! the caller (typically the arcan-anima session bootstrap or the
//! lifed admin plane).

use anima_core::error::AnimaResult;
use anima_core::event::{AnimaEventKind, BackendKind};
use anima_identity::custody::DidRotationEvent;
use chrono::Utc;

/// Wrap a [`DidRotationEvent`] (returned by `AnimaCustody::rotate`)
/// in the canonical `AnimaEventKind::IdentityRotated` variant ready
/// for Lago append.
///
/// Spec D L4-D10 — rotation is documented in the journal, not
/// implicit. Calling this is the "I/O boundary" referenced in the
/// `AnimaCustody::rotate` SPEC-D-DEVIATION block: the trait returns
/// the data, this helper turns it into an event payload, and the
/// caller appends it to lago.
pub fn write_rotation_event(rotation: &DidRotationEvent) -> AnimaResult<AnimaEventKind> {
    Ok(AnimaEventKind::IdentityRotated {
        old_did: rotation.old_did.clone(),
        new_did: rotation.new_did.clone(),
        rotation_proof_jws: rotation.rotation_proof_jws.clone(),
        rotated_at: rotation.rotated_at,
    })
}

/// Build an `anima.identity_revoked` event. Spec D D-Sub-E — emit
/// this when an identity is compromised or end-of-life. Once written,
/// the revocation cache + verifier path reject any signature by `did`.
pub fn write_revocation_event(did: impl Into<String>, reason: impl Into<String>) -> AnimaEventKind {
    AnimaEventKind::IdentityRevoked {
        did: did.into(),
        reason: reason.into(),
        revoked_at: Utc::now(),
    }
}

/// Build an `anima.custody_migrated` event. Spec D L4-D9 — documents
/// that custody moved between backends (e.g. user upgraded
/// `InProcessAnima` → `VaultTransitAnima`).
pub fn write_custody_migration_event(
    from_backend: BackendKind,
    to_backend: BackendKind,
    attestation: Option<String>,
) -> AnimaEventKind {
    AnimaEventKind::CustodyMigrated {
        from_backend,
        to_backend,
        attestation,
        migrated_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn rotation_event_round_trip_matches_input() {
        let rotation = DidRotationEvent {
            old_did: "did:key:z6MkOld".into(),
            new_did: "did:key:zDnNew".into(),
            rotation_proof_jws: "header.body.sig".into(),
            rotated_at: Utc.with_ymd_and_hms(2026, 5, 1, 12, 0, 0).unwrap(),
        };
        let event = write_rotation_event(&rotation).unwrap();
        match event {
            AnimaEventKind::IdentityRotated {
                old_did,
                new_did,
                rotation_proof_jws,
                rotated_at,
            } => {
                assert_eq!(old_did, "did:key:z6MkOld");
                assert_eq!(new_did, "did:key:zDnNew");
                assert_eq!(rotation_proof_jws, "header.body.sig");
                assert_eq!(rotated_at, rotation.rotated_at);
            }
            other => panic!("expected IdentityRotated, got {other:?}"),
        }
    }

    #[test]
    fn revocation_event_carries_did_and_reason() {
        let event = write_revocation_event("did:key:zDnGone", "device lost");
        match event {
            AnimaEventKind::IdentityRevoked { did, reason, .. } => {
                assert_eq!(did, "did:key:zDnGone");
                assert_eq!(reason, "device lost");
            }
            other => panic!("expected IdentityRevoked, got {other:?}"),
        }
    }

    #[test]
    fn custody_migration_event_records_both_backends() {
        let event = write_custody_migration_event(
            BackendKind::InProcess,
            BackendKind::Soma,
            Some("attest-1".into()),
        );
        match event {
            AnimaEventKind::CustodyMigrated {
                from_backend,
                to_backend,
                attestation,
                ..
            } => {
                assert_eq!(from_backend, BackendKind::InProcess);
                assert_eq!(to_backend, BackendKind::Soma);
                assert_eq!(attestation.as_deref(), Some("attest-1"));
            }
            other => panic!("expected CustodyMigrated, got {other:?}"),
        }
    }
}
