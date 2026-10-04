//! `NousLineage` — provenance trail for nous decisions.
//!
//! Every time a nous evaluator (heuristic, judge, scorer) runs against
//! some target, the substrate records a [`LineageEvent`]: which agent
//! ran, what input it saw, what output it emitted, when, against which
//! session, and which lago event (if any) the call traces back to.
//!
//! Authored agents in BRO-1011 (`nous-promoter`) and BRO-1012
//! (bookkeeping scorers) consume this trail through
//! [`NousLineage::query`] to reason over historical scores when
//! deciding whether a candidate has earned promotion.
//!
//! This crate ships the trait + an in-memory reference implementation
//! ([`InMemoryNousLineage`]). A lago-backed implementation that mirrors
//! events into the journal is intentionally **out of scope** for
//! BRO-1009 — it lands separately so the two crates can ship
//! independently.

use crate::error::NousLineageError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Opaque identifier returned by [`NousLineage::record`].
///
/// Backed by a string (rather than a typed ULID) so backends are free
/// to use their own conventions — the in-memory backend uses sequential
/// ULID-shaped strings, and a lago-backed backend would return the
/// `EventId` of the persisted event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LineageId(pub String);

impl LineageId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A single nous evaluation event with its full provenance.
///
/// Fields are intentionally JSON-shaped (`serde_json::Value` for
/// `input` and `output`) so different scorers can ship their own
/// schemas without forcing a typed enum migration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineageEvent {
    /// The agent session under which this evaluation ran. Mirrors
    /// `lago_core::SessionId` but kept as a `String` so this trait
    /// does not pin the lineage substrate to lago.
    pub session_id: String,
    /// Stable name of the scorer/judge that produced this row,
    /// e.g. `"bookkeeping.score-novelty"` or `"nous.promoter.judge"`.
    pub agent_name: String,
    /// Optional reference to the artifact that was scored. Free-form so
    /// callers can use whatever URI / path / wikilink shape suits their
    /// surface (e.g. `"research/notes/foo-raw.md#item-3"` or a lago
    /// `EventId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    /// The input payload the scorer received (shape is scorer-defined).
    pub input: serde_json::Value,
    /// The output payload the scorer emitted (shape is scorer-defined).
    pub output: serde_json::Value,
    /// Wall-clock timestamp of the event in milliseconds since epoch.
    pub timestamp_ms: u64,
    /// Optional pointer back to the lago event that triggered or
    /// recorded this evaluation. Lets downstream agents walk from
    /// lineage row → lago event → original artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lago_event_id: Option<String>,
}

/// Filter used by [`NousLineage::query`].
///
/// All fields are optional and combined with logical AND. An empty
/// filter returns every recorded event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LineageFilter {
    /// Restrict to a single session.
    pub session_id: Option<String>,
    /// Restrict to a single agent (e.g. `"bookkeeping.score-novelty"`).
    pub agent_name: Option<String>,
    /// Restrict to evaluations whose `target_ref` matches exactly.
    pub target_ref: Option<String>,
    /// Inclusive lower bound on `timestamp_ms`.
    pub after_ts_ms: Option<u64>,
    /// Inclusive upper bound on `timestamp_ms`.
    pub before_ts_ms: Option<u64>,
    /// Maximum number of events returned (no cap means "all matching").
    pub limit: Option<usize>,
}

impl LineageFilter {
    /// Returns true if `event` satisfies every set field.
    pub fn matches(&self, event: &LineageEvent) -> bool {
        if let Some(ref s) = self.session_id
            && &event.session_id != s
        {
            return false;
        }
        if let Some(ref a) = self.agent_name
            && &event.agent_name != a
        {
            return false;
        }
        if let Some(ref t) = self.target_ref
            && event.target_ref.as_ref() != Some(t)
        {
            return false;
        }
        if let Some(after) = self.after_ts_ms
            && event.timestamp_ms < after
        {
            return false;
        }
        if let Some(before) = self.before_ts_ms
            && event.timestamp_ms > before
        {
            return false;
        }
        true
    }
}

/// The provenance trail for nous decisions.
///
/// Implementations must preserve insertion order on read — downstream
/// promoters rely on chronological iteration to detect drift.
#[async_trait]
pub trait NousLineage: Send + Sync {
    /// Record a single scoring/judging event with its full provenance.
    async fn record(&self, event: LineageEvent) -> Result<LineageId, NousLineageError>;

    /// Query the lineage by aggregator key (e.g. all scores for entity X).
    async fn query(&self, filter: LineageFilter) -> Result<Vec<LineageEvent>, NousLineageError>;
}

/// In-memory reference implementation backed by an `Arc<RwLock<Vec<_>>>`.
///
/// Suitable for tests, fixtures, and short-lived single-process agents.
/// Events are appended in record-order and returned in the same order
/// from [`NousLineage::query`].
#[derive(Debug, Clone, Default)]
pub struct InMemoryNousLineage {
    inner: Arc<RwLock<Vec<(LineageId, LineageEvent)>>>,
}

impl InMemoryNousLineage {
    /// Construct an empty lineage store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of events currently recorded.
    ///
    /// Useful for assertions in tests.
    pub fn len(&self) -> usize {
        self.inner.read().map(|guard| guard.len()).unwrap_or(0)
    }

    /// Returns true if no events have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl NousLineage for InMemoryNousLineage {
    async fn record(&self, event: LineageEvent) -> Result<LineageId, NousLineageError> {
        let mut guard = self
            .inner
            .write()
            .map_err(|e| NousLineageError::Store(format!("write lock poisoned: {e}")))?;
        // Sequential id ("lineage-N") keeps the in-memory backend
        // dependency-free (no ulid). A lago-backed backend would
        // return the persisted EventId.
        let id = LineageId::new(format!("lineage-{}", guard.len()));
        guard.push((id.clone(), event));
        Ok(id)
    }

    async fn query(&self, filter: LineageFilter) -> Result<Vec<LineageEvent>, NousLineageError> {
        if let (Some(after), Some(before)) = (filter.after_ts_ms, filter.before_ts_ms)
            && after > before
        {
            return Err(NousLineageError::InvalidFilter(format!(
                "after_ts_ms ({after}) is greater than before_ts_ms ({before})"
            )));
        }

        let guard = self
            .inner
            .read()
            .map_err(|e| NousLineageError::Store(format!("read lock poisoned: {e}")))?;

        let mut out = Vec::new();
        for (_, ev) in guard.iter() {
            if filter.matches(ev) {
                out.push(ev.clone());
                if let Some(limit) = filter.limit
                    && out.len() >= limit
                {
                    break;
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(
        session: &str,
        agent: &str,
        target: Option<&str>,
        input: serde_json::Value,
        output: serde_json::Value,
        ts: u64,
    ) -> LineageEvent {
        LineageEvent {
            session_id: session.to_string(),
            agent_name: agent.to_string(),
            target_ref: target.map(str::to_string),
            input,
            output,
            timestamp_ms: ts,
            lago_event_id: None,
        }
    }

    #[tokio::test]
    async fn in_memory_nous_lineage_round_trip() {
        let store = InMemoryNousLineage::new();
        assert!(store.is_empty());

        let a = ev(
            "S1",
            "bookkeeping.score-novelty",
            Some("notes/a.md#1"),
            json!({"text": "alpha"}),
            json!({"score": 7}),
            100,
        );
        let b = ev(
            "S1",
            "bookkeeping.score-novelty",
            Some("notes/b.md#1"),
            json!({"text": "beta"}),
            json!({"score": 3}),
            200,
        );
        let c = ev(
            "S2",
            "bookkeeping.score-relevance",
            None,
            json!({"text": "gamma"}),
            json!({"score": 9}),
            300,
        );

        let id_a = store.record(a.clone()).await.unwrap();
        let id_b = store.record(b.clone()).await.unwrap();
        let id_c = store.record(c.clone()).await.unwrap();

        // Sequential ids preserve insertion order.
        assert_eq!(id_a.as_str(), "lineage-0");
        assert_eq!(id_b.as_str(), "lineage-1");
        assert_eq!(id_c.as_str(), "lineage-2");
        assert_eq!(store.len(), 3);

        // Empty filter returns all events in insertion order.
        let all = store.query(LineageFilter::default()).await.unwrap();
        assert_eq!(all, vec![a.clone(), b.clone(), c.clone()]);

        // Session filter.
        let s1 = store
            .query(LineageFilter {
                session_id: Some("S1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(s1, vec![a.clone(), b.clone()]);

        // Agent filter.
        let novelty = store
            .query(LineageFilter {
                agent_name: Some("bookkeeping.score-novelty".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(novelty, vec![a.clone(), b.clone()]);

        // Target filter.
        let only_a = store
            .query(LineageFilter {
                target_ref: Some("notes/a.md#1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(only_a, vec![a.clone()]);

        // Time window.
        let window = store
            .query(LineageFilter {
                after_ts_ms: Some(150),
                before_ts_ms: Some(250),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(window, vec![b.clone()]);

        // Limit.
        let one = store
            .query(LineageFilter {
                limit: Some(1),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(one, vec![a.clone()]);
    }

    #[tokio::test]
    async fn invalid_window_is_rejected() {
        let store = InMemoryNousLineage::new();
        let err = store
            .query(LineageFilter {
                after_ts_ms: Some(500),
                before_ts_ms: Some(100),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(err, NousLineageError::InvalidFilter(_)));
    }
}
