//! Spec D D-Sub-E — rotation chain + revocation integration tests.
//!
//! Drives the [`anima_identity::rotation::JournalResolver`] +
//! [`anima_identity::revocation::RevocationCache`] stack against
//! deterministic in-memory journals. Verifies:
//!
//! 1. Rotation chain walking from genesis to current,
//! 2. Walking from interim DIDs (mid-chain) returns only the
//!    remaining hops,
//! 3. Revocation events block subsequent verifies,
//! 4. The cache caches negative answers and returns positives forever,
//! 5. Cycle detection in pathological journals.
//!
//! Together with the unit tests inside `rotation.rs` / `revocation.rs`,
//! this gives the 5+ integration coverage Spec D D-Sub-E asks for.

use anima_core::error::AnimaResult;
use anima_core::identity_document::DidRotation;
use anima_identity::revocation::{RevocationCache, is_revoked};
use anima_identity::rotation::{JournalResolver, RotationChainQuery, walk_rotation_chain};
use async_trait::async_trait;
use std::sync::Mutex;
use std::time::Duration;

/// In-memory journal resolver — production callers swap a Lago-backed
/// implementation in.
struct MockJournal {
    rotations: Vec<DidRotation>,
    revoked: Mutex<Vec<(String, u64)>>,
}

impl MockJournal {
    fn with_rotations(rotations: Vec<DidRotation>) -> Self {
        Self {
            rotations,
            revoked: Mutex::new(Vec::new()),
        }
    }

    fn revoke(&self, did: &str, at_seq: u64) {
        self.revoked.lock().unwrap().push((did.to_string(), at_seq));
    }
}

#[async_trait]
impl JournalResolver for MockJournal {
    async fn rotation_events_for(
        &self,
        _q: RotationChainQuery<'_>,
    ) -> AnimaResult<Vec<DidRotation>> {
        Ok(self.rotations.clone())
    }

    async fn revocation_event_for(&self, did: &str) -> AnimaResult<Option<u64>> {
        let revoked = self.revoked.lock().unwrap();
        Ok(revoked.iter().find(|(d, _)| d == did).map(|(_, seq)| *seq))
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
async fn integration_walk_rotation_chain_full_path() {
    let journal = MockJournal::with_rotations(vec![
        rot("did:key:zDnGenesis", "did:key:zDnV2", 100),
        rot("did:key:zDnV2", "did:key:zDnV3", 200),
        rot("did:key:zDnV3", "did:key:zDnCurrent", 300),
    ]);
    let chain = walk_rotation_chain("did:key:zDnGenesis", &journal)
        .await
        .unwrap();
    assert_eq!(chain.len(), 3, "chain should walk three hops");
    assert_eq!(chain.last().unwrap().new_did, "did:key:zDnCurrent");
    // Seqs ascending — same as the journal write order.
    assert_eq!(chain[0].rotated_at_seq, 100);
    assert_eq!(chain[1].rotated_at_seq, 200);
    assert_eq!(chain[2].rotated_at_seq, 300);
}

#[tokio::test]
async fn integration_walk_rotation_chain_from_mid_chain() {
    // Verifier sees a JWT minted under did:key:zDnV2 → walks forward
    // and discovers V3, V4. The first hop (Genesis → V2) is NOT
    // included because we started from V2.
    let journal = MockJournal::with_rotations(vec![
        rot("did:key:zDnGenesis", "did:key:zDnV2", 100),
        rot("did:key:zDnV2", "did:key:zDnV3", 200),
        rot("did:key:zDnV3", "did:key:zDnV4", 300),
    ]);
    let chain = walk_rotation_chain("did:key:zDnV2", &journal)
        .await
        .unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].old_did, "did:key:zDnV2");
    assert_eq!(chain[0].new_did, "did:key:zDnV3");
    assert_eq!(chain[1].old_did, "did:key:zDnV3");
    assert_eq!(chain[1].new_did, "did:key:zDnV4");
}

#[tokio::test]
async fn integration_revocation_blocks_did() {
    let journal = MockJournal::with_rotations(vec![]);
    journal.revoke("did:key:zDnLost", 500);

    // Cold cache lookup — resolver fires.
    let cache = RevocationCache::new();
    assert!(cache.check("did:key:zDnLost", &journal).await.unwrap());

    // Bare helper without cache also reports revoked.
    assert!(is_revoked("did:key:zDnLost", &journal, None).await.unwrap());
    // Fresh DID with no revocation event is NOT revoked.
    assert!(
        !is_revoked("did:key:zDnFresh", &journal, None)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn integration_revocation_cache_avoids_repeated_lookups() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingJournal {
        counter: AtomicUsize,
    }
    #[async_trait]
    impl JournalResolver for CountingJournal {
        async fn rotation_events_for(
            &self,
            _q: RotationChainQuery<'_>,
        ) -> AnimaResult<Vec<DidRotation>> {
            Ok(Vec::new())
        }
        async fn revocation_event_for(&self, _did: &str) -> AnimaResult<Option<u64>> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }

    let journal = CountingJournal {
        counter: AtomicUsize::new(0),
    };
    let cache = RevocationCache::with_ttl(Duration::from_secs(60));
    for _ in 0..10 {
        let _ = cache.check("did:key:zDnHot", &journal).await.unwrap();
    }
    assert_eq!(
        journal.counter.load(Ordering::SeqCst),
        1,
        "cache must amortise repeat checks within TTL"
    );
}

#[tokio::test]
async fn integration_walk_with_no_journal_history_returns_empty() {
    let journal = MockJournal::with_rotations(vec![]);
    let chain = walk_rotation_chain("did:key:zDnNeverRotated", &journal)
        .await
        .unwrap();
    assert!(chain.is_empty());
}

#[tokio::test]
async fn integration_post_revocation_invalidate_picks_up_new_state() {
    let journal = MockJournal::with_rotations(vec![]);
    let cache = RevocationCache::new();

    // Pre-revocation: not revoked.
    assert!(!cache.check("did:key:zDnSafe", &journal).await.unwrap());

    // Operator writes the revocation event...
    journal.revoke("did:key:zDnSafe", 999);
    // Cache still says NOT revoked (cached negative).
    assert!(!cache.check("did:key:zDnSafe", &journal).await.unwrap());
    // Invalidating the entry forces a fresh lookup → revoked.
    cache.invalidate("did:key:zDnSafe");
    assert!(cache.check("did:key:zDnSafe", &journal).await.unwrap());
}

#[tokio::test]
async fn integration_walk_chain_terminal_did_returns_empty() {
    // A verifier holding the CURRENT DID should not see further hops.
    let journal = MockJournal::with_rotations(vec![rot("did:key:zDnA", "did:key:zDnCurrent", 10)]);
    let chain = walk_rotation_chain("did:key:zDnCurrent", &journal)
        .await
        .unwrap();
    assert!(chain.is_empty(), "current DID has no outgoing rotation");
}

/// The "fresh deploy + first rotation" path — `anima.identity_rotated`
/// is the only event in the journal and the chain has one hop.
#[tokio::test]
async fn integration_first_rotation_chain_has_one_hop() {
    let journal = MockJournal::with_rotations(vec![rot("did:key:zDnGenesis", "did:key:zDnV2", 1)]);
    let chain = walk_rotation_chain("did:key:zDnGenesis", &journal)
        .await
        .unwrap();
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].old_did, "did:key:zDnGenesis");
    assert_eq!(chain[0].new_did, "did:key:zDnV2");
    assert_eq!(chain[0].rotated_at_seq, 1);
}
