//! Soul genesis — how an agent's soul enters the world.
//!
//! The genesis event is the first event in an agent's Lago journal.
//! It contains the complete serialized AgentSoul. Once written,
//! it can never be overwritten or modified (append-only guarantee).
//!
//! This module provides functions to create genesis events and
//! reconstruct souls from journal replay.

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::event::AnimaEventKind;
use anima_core::soul::AgentSoul;

/// Create the genesis event data for a soul.
///
/// This produces the `AnimaEventKind::SoulGenesis` variant that
/// should be wrapped in an `EventEnvelope` and appended to Lago.
pub fn create_genesis_event(soul: &AgentSoul) -> AnimaResult<AnimaEventKind> {
    let soul_json = serde_json::to_value(soul)?;

    Ok(AnimaEventKind::SoulGenesis {
        soul: soul_json,
        soul_hash: soul.soul_hash().to_string(),
    })
}

/// Reconstruct a soul from a genesis event.
///
/// Deserializes the soul from the event data and verifies its
/// integrity hash matches.
pub fn reconstruct_soul(event: &AnimaEventKind) -> AnimaResult<AgentSoul> {
    match event {
        AnimaEventKind::SoulGenesis { soul, soul_hash } => {
            let deserialized: AgentSoul = serde_json::from_value(soul.clone())?;

            // Verify the stored hash matches the computed hash
            if deserialized.soul_hash() != soul_hash {
                return Err(AnimaError::SoulIntegrityViolation {
                    expected: soul_hash.clone(),
                    actual: deserialized.soul_hash().to_string(),
                });
            }

            // Verify internal integrity
            deserialized.verify_integrity()?;

            Ok(deserialized)
        }
        _ => Err(AnimaError::SoulNotFound {
            agent_id: "unknown".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anima_core::soul::SoulBuilder;

    #[test]
    fn genesis_roundtrip() {
        let soul = SoulBuilder::new("test-agent", "test mission", vec![1u8; 32]).build();
        let event = create_genesis_event(&soul).unwrap();
        let recovered = reconstruct_soul(&event).unwrap();

        assert_eq!(soul, recovered);
        assert!(recovered.verify_integrity().is_ok());
    }

    #[test]
    fn wrong_event_type_fails() {
        let event = AnimaEventKind::CapabilityGranted {
            capability: "chat:send".into(),
            granted_by: "server".into(),
            expires_at: None,
            constraints: serde_json::json!({}),
        };

        assert!(reconstruct_soul(&event).is_err());
    }
}
