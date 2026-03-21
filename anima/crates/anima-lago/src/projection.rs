//! Belief projection — deterministic fold over the event stream.
//!
//! AgentBelief is a pure projection: given the same sequence of events,
//! you always get the same belief state. This module implements the
//! fold function that reduces a stream of AnimaEventKind events into
//! an AgentBelief.
//!
//! This follows the same pattern as:
//! - Haima's `FinancialState::apply()`
//! - Autonomic's `projection::fold()`

use anima_core::belief::{
    ActiveConstraint, AgentBelief, ConstraintSource, EconomicBelief, GrantedCapability, TrustScore,
};
use anima_core::event::AnimaEventKind;
use chrono::{DateTime, Utc};

/// Apply a single Anima event to the belief state.
///
/// This is the core fold function. It is pure — no I/O, no side
/// effects, deterministic output for any given input.
pub fn fold(belief: &mut AgentBelief, event: &AnimaEventKind, seq: u64, timestamp: DateTime<Utc>) {
    belief.last_event_seq = seq;
    belief.last_updated = timestamp;

    match event {
        AnimaEventKind::CapabilityGranted {
            capability,
            granted_by,
            expires_at,
            constraints: _,
        } => {
            belief.capabilities.push(GrantedCapability {
                capability: capability.clone(),
                granted_by: granted_by.clone(),
                granted_at: timestamp,
                expires_at: *expires_at,
                constraints: vec![], // Constraints parsed from JSON if needed
            });
        }

        AnimaEventKind::CapabilityRevoked {
            capability,
            revoked_by: _,
            reason: _,
        } => {
            belief.capabilities.retain(|c| c.capability != *capability);
        }

        AnimaEventKind::TrustUpdated {
            peer_id,
            new_score,
            interaction_success,
        } => {
            let entry = belief
                .trust_scores
                .entry(peer_id.clone())
                .or_insert(TrustScore {
                    score: 0.5,
                    successful_interactions: 0,
                    failed_interactions: 0,
                    last_interaction: timestamp,
                });

            entry.score = *new_score;
            entry.last_interaction = timestamp;
            if *interaction_success {
                entry.successful_interactions += 1;
            } else {
                entry.failed_interactions += 1;
            }
        }

        AnimaEventKind::EconomicBeliefUpdated {
            balance_micro_credits,
            burn_rate_per_hour,
            economic_mode,
        } => {
            belief.economic_belief = EconomicBelief {
                balance_micro_credits: *balance_micro_credits,
                burn_rate_per_hour: *burn_rate_per_hour,
                hours_until_exhaustion: if *burn_rate_per_hour > 0.0 {
                    Some(*balance_micro_credits as f64 / burn_rate_per_hour)
                } else {
                    None
                },
                economic_mode: economic_mode.clone(),
                session_spend_micro_credits: belief.economic_belief.session_spend_micro_credits,
            };
        }

        AnimaEventKind::BeliefSnapshot {
            belief: snapshot, ..
        } => {
            // A snapshot replaces the entire belief state.
            // This is used for fast recovery — instead of replaying
            // all events, load the latest snapshot and replay from there.
            if let Ok(restored) = serde_json::from_value(snapshot.clone()) {
                *belief = restored;
                belief.last_event_seq = seq;
                belief.last_updated = timestamp;
            }
        }

        AnimaEventKind::PolicyViolationDetected {
            capability,
            reason,
            blocked,
        } => {
            if *blocked {
                // If a policy violation was blocked, add a constraint
                belief.constraints.push(ActiveConstraint {
                    id: format!("violation-{seq}"),
                    source: ConstraintSource::Soul,
                    description: format!("blocked: capability '{}' — {}", capability, reason),
                    imposed_at: timestamp,
                });
            }

            // Update reputation (violations are visible)
            belief.reputation.violations += 1;
        }

        // Events that don't directly modify beliefs
        AnimaEventKind::SoulGenesis { .. }
        | AnimaEventKind::IdentityCreated { .. }
        | AnimaEventKind::IdentityTransitioned { .. }
        | AnimaEventKind::KeyRotated { .. }
        | AnimaEventKind::LineageVerified { .. } => {
            // These events are tracked but don't change beliefs.
            // They affect Soul and Identity, which are managed separately.
        }
    }
}

/// Replay a sequence of events to reconstruct belief state.
///
/// Starts from a default (empty) belief and folds all events.
pub fn replay(events: &[(AnimaEventKind, u64, DateTime<Utc>)]) -> AgentBelief {
    let mut belief = AgentBelief::default();
    for (event, seq, ts) in events {
        fold(&mut belief, event, *seq, *ts);
    }
    belief
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_grants_capabilities() {
        let now = Utc::now();
        let events = vec![
            (
                AnimaEventKind::CapabilityGranted {
                    capability: "chat:send".into(),
                    granted_by: "server-1".into(),
                    expires_at: None,
                    constraints: serde_json::json!({}),
                },
                1,
                now,
            ),
            (
                AnimaEventKind::CapabilityGranted {
                    capability: "knowledge:read".into(),
                    granted_by: "server-1".into(),
                    expires_at: None,
                    constraints: serde_json::json!({}),
                },
                2,
                now,
            ),
        ];

        let belief = replay(&events);
        assert_eq!(belief.capabilities.len(), 2);
        assert!(belief.has_capability("chat:send"));
        assert!(belief.has_capability("knowledge:read"));
    }

    #[test]
    fn replay_revokes_capabilities() {
        let now = Utc::now();
        let events = vec![
            (
                AnimaEventKind::CapabilityGranted {
                    capability: "chat:send".into(),
                    granted_by: "server-1".into(),
                    expires_at: None,
                    constraints: serde_json::json!({}),
                },
                1,
                now,
            ),
            (
                AnimaEventKind::CapabilityRevoked {
                    capability: "chat:send".into(),
                    revoked_by: "server-1".into(),
                    reason: "expired".into(),
                },
                2,
                now,
            ),
        ];

        let belief = replay(&events);
        assert!(!belief.has_capability("chat:send"));
    }

    #[test]
    fn replay_tracks_trust() {
        let now = Utc::now();
        let events = vec![
            (
                AnimaEventKind::TrustUpdated {
                    peer_id: "peer-1".into(),
                    new_score: 0.8,
                    interaction_success: true,
                },
                1,
                now,
            ),
            (
                AnimaEventKind::TrustUpdated {
                    peer_id: "peer-1".into(),
                    new_score: 0.6,
                    interaction_success: false,
                },
                2,
                now,
            ),
        ];

        let belief = replay(&events);
        let trust = &belief.trust_scores["peer-1"];
        assert!((trust.score - 0.6).abs() < f64::EPSILON);
        assert_eq!(trust.successful_interactions, 1);
        assert_eq!(trust.failed_interactions, 1);
    }

    #[test]
    fn replay_updates_economics() {
        let now = Utc::now();
        let events = vec![(
            AnimaEventKind::EconomicBeliefUpdated {
                balance_micro_credits: 5_000_000,
                burn_rate_per_hour: 100_000.0,
                economic_mode: "conserving".into(),
            },
            1,
            now,
        )];

        let belief = replay(&events);
        assert_eq!(belief.economic_belief.balance_micro_credits, 5_000_000);
        assert_eq!(belief.economic_belief.economic_mode, "conserving");
        assert!(belief.economic_belief.hours_until_exhaustion.is_some());
    }

    #[test]
    fn snapshot_replaces_state() {
        let now = Utc::now();

        // Build up some state
        let mut belief = AgentBelief::default();
        belief.reputation.tasks_completed = 42;

        let snapshot = serde_json::to_value(&belief).unwrap();
        let hash = blake3::hash(serde_json::to_string(&belief).unwrap().as_bytes())
            .to_hex()
            .to_string();

        // Start fresh and replay with snapshot
        let events = vec![(
            AnimaEventKind::BeliefSnapshot {
                belief: snapshot,
                snapshot_hash: hash,
            },
            100,
            now,
        )];

        let recovered = replay(&events);
        assert_eq!(recovered.reputation.tasks_completed, 42);
    }

    #[test]
    fn policy_violation_adds_constraint() {
        let now = Utc::now();
        let events = vec![(
            AnimaEventKind::PolicyViolationDetected {
                capability: "admin:delete".into(),
                reason: "exceeds ceiling".into(),
                blocked: true,
            },
            1,
            now,
        )];

        let belief = replay(&events);
        assert_eq!(belief.constraints.len(), 1);
        assert_eq!(belief.reputation.violations, 1);
    }
}
