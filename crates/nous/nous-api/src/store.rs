//! Thread-safe in-memory store for evaluation scores.
//!
//! `ScoreStore` holds scores indexed by session ID using an `Arc<RwLock<...>>`
//! pattern so it can be shared across axum handlers via `State`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use nous_core::EvalScore;

/// Thread-safe in-memory store for evaluation scores, indexed by session ID.
#[derive(Clone, Default)]
pub struct ScoreStore {
    inner: Arc<RwLock<HashMap<String, Vec<EvalScore>>>>,
}

impl ScoreStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a single score for its session.
    pub fn record(&self, score: EvalScore) {
        let mut map = self.inner.write().expect("ScoreStore lock poisoned");
        map.entry(score.session_id.clone()).or_default().push(score);
    }

    /// Record multiple scores.
    pub fn record_batch(&self, scores: Vec<EvalScore>) {
        let mut map = self.inner.write().expect("ScoreStore lock poisoned");
        for score in scores {
            map.entry(score.session_id.clone()).or_default().push(score);
        }
    }

    /// Get all scores for a session (empty vec if unknown).
    pub fn get_session_scores(&self, session_id: &str) -> Vec<EvalScore> {
        let map = self.inner.read().expect("ScoreStore lock poisoned");
        map.get(session_id).cloned().unwrap_or_default()
    }

    /// Aggregate quality score for a session (mean of all score values).
    ///
    /// Returns 0.0 if the session has no scores.
    pub fn aggregate_quality(&self, session_id: &str) -> f64 {
        let map = self.inner.read().expect("ScoreStore lock poisoned");
        match map.get(session_id) {
            Some(scores) if !scores.is_empty() => {
                let sum: f64 = scores.iter().map(|s| s.value).sum();
                sum / scores.len() as f64
            }
            _ => 0.0,
        }
    }

    /// Get all known session IDs.
    pub fn session_ids(&self) -> Vec<String> {
        let map = self.inner.read().expect("ScoreStore lock poisoned");
        map.keys().cloned().collect()
    }

    /// Total number of scores across all sessions.
    pub fn total_score_count(&self) -> usize {
        let map = self.inner.read().expect("ScoreStore lock poisoned");
        map.values().map(Vec::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::{EvalLayer, EvalScore, EvalTiming};

    fn make_score(evaluator: &str, value: f64, session_id: &str) -> EvalScore {
        EvalScore::new(
            evaluator,
            value,
            EvalLayer::Execution,
            EvalTiming::Inline,
            session_id,
        )
        .unwrap()
    }

    #[test]
    fn record_and_retrieve() {
        let store = ScoreStore::new();
        let score = make_score("token_efficiency", 0.85, "sess-1");
        store.record(score);

        let scores = store.get_session_scores("sess-1");
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].evaluator, "token_efficiency");
        assert!((scores[0].value - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn record_batch_and_retrieve() {
        let store = ScoreStore::new();
        let scores = vec![
            make_score("a", 0.8, "sess-1"),
            make_score("b", 0.6, "sess-1"),
            make_score("c", 0.9, "sess-2"),
        ];
        store.record_batch(scores);

        assert_eq!(store.get_session_scores("sess-1").len(), 2);
        assert_eq!(store.get_session_scores("sess-2").len(), 1);
    }

    #[test]
    fn aggregate_quality_calculation() {
        let store = ScoreStore::new();
        store.record(make_score("a", 0.8, "sess-1"));
        store.record(make_score("b", 0.6, "sess-1"));

        let agg = store.aggregate_quality("sess-1");
        assert!((agg - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn aggregate_quality_unknown_session() {
        let store = ScoreStore::new();
        assert!((store.aggregate_quality("no-such")).abs() < f64::EPSILON);
    }

    #[test]
    fn multiple_sessions_isolated() {
        let store = ScoreStore::new();
        store.record(make_score("a", 0.9, "s1"));
        store.record(make_score("b", 0.3, "s2"));

        let s1_scores = store.get_session_scores("s1");
        let s2_scores = store.get_session_scores("s2");
        assert_eq!(s1_scores.len(), 1);
        assert_eq!(s2_scores.len(), 1);
        assert!((s1_scores[0].value - 0.9).abs() < f64::EPSILON);
        assert!((s2_scores[0].value - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn session_ids_listed() {
        let store = ScoreStore::new();
        store.record(make_score("a", 0.5, "alpha"));
        store.record(make_score("b", 0.5, "beta"));

        let mut ids = store.session_ids();
        ids.sort();
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn total_score_count() {
        let store = ScoreStore::new();
        assert_eq!(store.total_score_count(), 0);

        store.record(make_score("a", 0.5, "s1"));
        store.record(make_score("b", 0.5, "s1"));
        store.record(make_score("c", 0.5, "s2"));
        assert_eq!(store.total_score_count(), 3);
    }

    #[test]
    fn unknown_session_returns_empty() {
        let store = ScoreStore::new();
        assert!(store.get_session_scores("nonexistent").is_empty());
    }

    #[test]
    fn thread_safety_concurrent_record() {
        use std::thread;

        let store = ScoreStore::new();
        let mut handles = vec![];

        for i in 0..10 {
            let s = store.clone();
            handles.push(thread::spawn(move || {
                let score = make_score("eval", 0.5, &format!("sess-{i}"));
                s.record(score);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(store.total_score_count(), 10);
        assert_eq!(store.session_ids().len(), 10);
    }
}
