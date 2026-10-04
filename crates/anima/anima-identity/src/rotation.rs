//! Rotation chain walking (Spec D D-Sub-E).
//!
//! Spec D §"Event additions" defines the verifier semantics for DID
//! rotation: every `anima.identity_rotated { old_did, new_did, …,
//! rotated_at_seq }` event in the Lago journal extends a chain from
//! the genesis DID to the currently authoritative DID. Verifiers
//! seeing an old DID resolve back through this chain to discover the
//! new DID and pull the verifying key from there.
//!
//! This module factors that walk into a backend-agnostic helper. The
//! [`JournalResolver`] async trait abstracts over the underlying
//! storage so both lago-auth's production verifier and the test
//! harness can drive it with their own implementations:
//!
//! ```rust,no_run
//! use anima_identity::rotation::{JournalResolver, RotationChainQuery, walk_rotation_chain};
//! use anima_core::error::AnimaResult;
//! use anima_core::identity_document::DidRotation;
//!
//! struct MyResolver;
//! #[async_trait::async_trait]
//! impl JournalResolver for MyResolver {
//!     async fn rotation_events_for(
//!         &self,
//!         _query: RotationChainQuery<'_>,
//!     ) -> AnimaResult<Vec<DidRotation>> {
//!         Ok(vec![]) // your implementation talks to lago / replays events here
//!     }
//!     async fn revocation_event_for(&self, _did: &str) -> AnimaResult<Option<u64>> {
//!         Ok(None)
//!     }
//! }
//!
//! # async fn demo() {
//! let resolver = MyResolver;
//! let chain = walk_rotation_chain("did:key:z6MkOld", &resolver).await.unwrap();
//! # let _ = chain;
//! # }
//! ```

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity_document::DidRotation;
use async_trait::async_trait;

/// Query parameters for the rotation chain lookup.
///
/// `starting_did` is the DID the caller is currently looking at —
/// might be the genesis DID, an interim DID, or the current
/// authoritative DID. Implementations walk forward from there until
/// no further `anima.identity_rotated` event names this DID as
/// `old_did`.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RotationChainQuery<'a> {
    /// DID to start the walk from.
    pub starting_did: &'a str,
}

/// Backend-agnostic resolver over the Lago journal.
///
/// Production: implemented by lago-auth's `LagoJournalResolver` (which
/// replays `anima.identity_rotated` + `anima.identity_revoked` events
/// from a `lago_journal::RedbJournal`). Tests: implemented by the
/// in-memory mock fixture.
///
/// `#[async_trait]` is used because this is dyn-compatible — verifier
/// callers hold `&dyn JournalResolver` so the resolver can be swapped
/// at runtime (e.g. between staging Vault-backed lago and a local
/// fixture).
#[async_trait]
pub trait JournalResolver: Send + Sync {
    /// Return all `anima.identity_rotated` events that mention
    /// `query.starting_did` in the rotation chain (either as `old_did`
    /// or as `new_did`). Implementations MAY also return earlier
    /// rotations so the caller can build a full ancestor chain.
    ///
    /// Order: ascending by `rotated_at_seq` (oldest first). The
    /// [`walk_rotation_chain`] helper depends on this ordering.
    async fn rotation_events_for(
        &self,
        query: RotationChainQuery<'_>,
    ) -> AnimaResult<Vec<DidRotation>>;

    /// If the DID is revoked, return the seq at which the
    /// `anima.identity_revoked` event was written. `None` means the
    /// DID is currently resolvable.
    ///
    /// This is the I/O boundary for revocation; the in-process
    /// caching layer in `crate::revocation::RevocationCache` decides
    /// when to actually call this.
    async fn revocation_event_for(&self, did: &str) -> AnimaResult<Option<u64>>;
}

/// Walk the rotation chain forward from `starting_did` until we hit
/// the currently authoritative DID (the one whose `new_did` is not
/// itself the `old_did` of any later event).
///
/// Returns the chain in journal order (oldest rotation first). The
/// resulting Vec is the same shape that
/// [`anima_core::identity_document::AgentIdentityDocument::rotation_chain`]
/// stores, so callers can hand it directly to
/// [`anima_core::identity_document::IdentityDocumentBuilder::rotation_chain`].
///
/// Empty chain means the DID has never rotated — the caller is
/// holding the genesis DID and can resolve directly.
///
/// ## Cycle protection
///
/// The walker bounds at 256 hops to bail out of pathological journals
/// (a malicious or corrupt journal that links DIDs in a loop). 256 is
/// well above any plausible production rotation rate (a hop a day for
/// 8 months would still fit) but tight enough to fail-fast on bugs.
pub async fn walk_rotation_chain(
    starting_did: &str,
    resolver: &dyn JournalResolver,
) -> AnimaResult<Vec<DidRotation>> {
    const MAX_HOPS: usize = 256;
    let events = resolver
        .rotation_events_for(RotationChainQuery { starting_did })
        .await?;

    if events.is_empty() {
        return Ok(Vec::new());
    }

    // Build a forward index: old_did -> &DidRotation. Then walk from
    // the starting DID forward, collecting rotations along the way.
    use std::collections::HashMap;
    let by_old: HashMap<&str, &DidRotation> =
        events.iter().map(|r| (r.old_did.as_str(), r)).collect();

    let mut chain = Vec::new();
    let mut cursor = starting_did;
    let mut hops = 0usize;
    while let Some(rot) = by_old.get(cursor) {
        if hops >= MAX_HOPS {
            return Err(AnimaError::Crypto(format!(
                "rotation chain exceeded {MAX_HOPS} hops from {starting_did} \
                 (cycle in journal?)"
            )));
        }
        chain.push((*rot).clone());
        cursor = rot.new_did.as_str();
        hops += 1;
    }
    Ok(chain)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory mock resolver for unit tests.
    struct MockResolver {
        events: Vec<DidRotation>,
        revoked: Vec<(String, u64)>,
    }

    #[async_trait]
    impl JournalResolver for MockResolver {
        async fn rotation_events_for(
            &self,
            _query: RotationChainQuery<'_>,
        ) -> AnimaResult<Vec<DidRotation>> {
            Ok(self.events.clone())
        }

        async fn revocation_event_for(&self, did: &str) -> AnimaResult<Option<u64>> {
            Ok(self
                .revoked
                .iter()
                .find(|(d, _)| d == did)
                .map(|(_, seq)| *seq))
        }
    }

    fn rot(old: &str, new: &str, seq: u64) -> DidRotation {
        DidRotation {
            old_did: old.into(),
            new_did: new.into(),
            rotation_proof_jws: format!("proof.{old}.{new}"),
            rotated_at_seq: seq,
        }
    }

    #[tokio::test]
    async fn walk_empty_journal_returns_empty_chain() {
        let resolver = MockResolver {
            events: vec![],
            revoked: vec![],
        };
        let chain = walk_rotation_chain("did:key:zDnAlone", &resolver)
            .await
            .unwrap();
        assert!(chain.is_empty());
    }

    #[tokio::test]
    async fn walk_single_rotation_returns_one_link() {
        let resolver = MockResolver {
            events: vec![rot("did:key:zDnA", "did:key:zDnB", 10)],
            revoked: vec![],
        };
        let chain = walk_rotation_chain("did:key:zDnA", &resolver)
            .await
            .unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].old_did, "did:key:zDnA");
        assert_eq!(chain[0].new_did, "did:key:zDnB");
    }

    #[tokio::test]
    async fn walk_chain_from_genesis_to_current() {
        let resolver = MockResolver {
            events: vec![
                rot("did:key:zDnA", "did:key:zDnB", 10),
                rot("did:key:zDnB", "did:key:zDnC", 20),
                rot("did:key:zDnC", "did:key:zDnD", 30),
            ],
            revoked: vec![],
        };
        let chain = walk_rotation_chain("did:key:zDnA", &resolver)
            .await
            .unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].new_did, "did:key:zDnB");
        assert_eq!(chain[1].new_did, "did:key:zDnC");
        assert_eq!(chain[2].new_did, "did:key:zDnD");
    }

    #[tokio::test]
    async fn walk_starting_at_interim_did_returns_remaining_links() {
        let resolver = MockResolver {
            events: vec![
                rot("did:key:zDnA", "did:key:zDnB", 10),
                rot("did:key:zDnB", "did:key:zDnC", 20),
                rot("did:key:zDnC", "did:key:zDnD", 30),
            ],
            revoked: vec![],
        };
        let chain = walk_rotation_chain("did:key:zDnB", &resolver)
            .await
            .unwrap();
        // Starting at B → walks B→C→D. The A→B link is NOT included
        // because the cursor begins at B.
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].old_did, "did:key:zDnB");
        assert_eq!(chain[1].new_did, "did:key:zDnD");
    }

    #[tokio::test]
    async fn walk_terminal_did_returns_empty() {
        let resolver = MockResolver {
            events: vec![rot("did:key:zDnA", "did:key:zDnB", 10)],
            revoked: vec![],
        };
        // No outgoing rotation from B → empty chain (B is current).
        let chain = walk_rotation_chain("did:key:zDnB", &resolver)
            .await
            .unwrap();
        assert!(chain.is_empty());
    }

    #[tokio::test]
    async fn walk_breaks_on_cycles() {
        // Construct a pathological journal where A→B→A.
        let resolver = MockResolver {
            events: vec![
                rot("did:key:zDnA", "did:key:zDnB", 10),
                rot("did:key:zDnB", "did:key:zDnA", 20),
            ],
            revoked: vec![],
        };
        let outcome = walk_rotation_chain("did:key:zDnA", &resolver).await;
        assert!(outcome.is_err(), "cycles must be detected");
        let msg = outcome.unwrap_err().to_string();
        assert!(msg.contains("256 hops"), "error mentions hop limit: {msg}");
    }
}
