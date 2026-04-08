//! Knowledge quality evaluators.
//!
//! Evaluate the freshness, coherence, and coverage of the agent's
//! knowledge substrate (backed by `lago-knowledge::KnowledgeIndex`).
//! These run at the `OnRunFinished` hook.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use lago_knowledge::KnowledgeIndex;
use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

// ---------------------------------------------------------------------------
// KnowledgeFreshnessEvaluator
// ---------------------------------------------------------------------------

/// Evaluates whether the knowledge index is fresh enough for reliable use.
///
/// Score interpretation:
/// - 1.0: index is fresh (built within `stale_threshold`)
/// - 0.5: index exists but is stale
/// - 0.0: index is empty
pub struct KnowledgeFreshnessEvaluator {
    index: Arc<RwLock<KnowledgeIndex>>,
    stale_threshold: Duration,
}

impl KnowledgeFreshnessEvaluator {
    /// Create with a custom stale threshold.
    pub fn new(index: Arc<RwLock<KnowledgeIndex>>, stale_threshold: Duration) -> Self {
        Self {
            index,
            stale_threshold,
        }
    }

    /// Create with the default 1-hour stale threshold.
    pub fn with_defaults(index: Arc<RwLock<KnowledgeIndex>>) -> Self {
        Self::new(index, Duration::from_secs(3600))
    }
}

impl NousEvaluator for KnowledgeFreshnessEvaluator {
    fn name(&self) -> &str {
        "knowledge_freshness"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Inline
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let guard = match self.index.read() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("knowledge_freshness: failed to acquire read lock");
                return Ok(vec![]);
            }
        };

        if guard.is_empty() {
            let score = EvalScore::new(
                self.name(),
                0.0,
                self.layer(),
                self.timing(),
                &ctx.session_id,
            )?
            .with_explanation("Knowledge index is empty (0 notes)".to_string());
            return Ok(vec![score]);
        }

        let note_count = guard.len();
        let is_stale = guard.is_stale(self.stale_threshold);
        let value = if is_stale { 0.5 } else { 1.0 };

        let freshness_label = if is_stale { "stale" } else { "fresh" };
        let explanation = format!(
            "Knowledge index has {note_count} notes, {freshness_label} (threshold: {}s)",
            self.stale_threshold.as_secs()
        );

        let score = EvalScore::new(
            self.name(),
            value,
            self.layer(),
            self.timing(),
            &ctx.session_id,
        )?
        .with_explanation(explanation);

        Ok(vec![score])
    }
}

// ---------------------------------------------------------------------------
// KnowledgeCoherenceEvaluator
// ---------------------------------------------------------------------------

/// Evaluates the structural coherence of the knowledge index via lint.
///
/// Score = `report.health_score` (0.0 = many issues, 1.0 = perfect).
/// Explanation includes contradiction and broken-link counts.
pub struct KnowledgeCoherenceEvaluator {
    index: Arc<RwLock<KnowledgeIndex>>,
}

impl KnowledgeCoherenceEvaluator {
    pub fn new(index: Arc<RwLock<KnowledgeIndex>>) -> Self {
        Self { index }
    }
}

impl NousEvaluator for KnowledgeCoherenceEvaluator {
    fn name(&self) -> &str {
        "knowledge_coherence"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Inline
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let guard = match self.index.read() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("knowledge_coherence: failed to acquire read lock");
                return Ok(vec![]);
            }
        };

        let report = guard.lint();
        let value = report.health_score as f64;

        let explanation = format!(
            "Lint: {} contradictions, {} broken links, {} orphans, {} stale claims (health {:.2})",
            report.contradictions.len(),
            report.broken_links.len(),
            report.orphan_pages.len(),
            report.stale_claims.len(),
            report.health_score
        );

        let score = EvalScore::new(
            self.name(),
            value,
            self.layer(),
            self.timing(),
            &ctx.session_id,
        )?
        .with_explanation(explanation);

        Ok(vec![score])
    }
}

// ---------------------------------------------------------------------------
// KnowledgeCoverageEvaluator
// ---------------------------------------------------------------------------

/// Evaluates knowledge coverage: how many referenced concepts have pages.
///
/// Score = 1.0 - (missing_pages / (total_notes + missing_pages)).
/// Explanation lists the top 3 missing pages.
pub struct KnowledgeCoverageEvaluator {
    index: Arc<RwLock<KnowledgeIndex>>,
}

impl KnowledgeCoverageEvaluator {
    pub fn new(index: Arc<RwLock<KnowledgeIndex>>) -> Self {
        Self { index }
    }
}

impl NousEvaluator for KnowledgeCoverageEvaluator {
    fn name(&self) -> &str {
        "knowledge_coverage"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Inline
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let guard = match self.index.read() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("knowledge_coverage: failed to acquire read lock");
                return Ok(vec![]);
            }
        };

        let report = guard.lint();
        let total_notes = guard.len();
        let missing_count = report.missing_pages.len();

        let denominator = total_notes + missing_count;
        let value = if denominator == 0 {
            1.0 // No notes and no missing pages — vacuously covered
        } else {
            1.0 - (missing_count as f64 / denominator as f64)
        };

        let top_missing: Vec<&str> = report
            .missing_pages
            .iter()
            .take(3)
            .map(String::as_str)
            .collect();

        let explanation = if top_missing.is_empty() {
            format!("All {total_notes} referenced concepts have pages")
        } else {
            format!(
                "{missing_count} missing pages out of {denominator} total concepts; top missing: {}",
                top_missing.join(", ")
            )
        };

        let score = EvalScore::new(
            self.name(),
            value,
            self.layer(),
            self.timing(),
            &ctx.session_id,
        )?
        .with_explanation(explanation);

        Ok(vec![score])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lago_core::ManifestEntry;
    use lago_store::BlobStore;
    use tempfile::TempDir;

    /// Helper: build a `KnowledgeIndex` from in-memory files.
    fn build_index(files: &[(&str, &str)]) -> (TempDir, Arc<RwLock<KnowledgeIndex>>) {
        let tmp = TempDir::new().unwrap();
        let store = BlobStore::open(tmp.path()).unwrap();
        let mut entries = Vec::new();

        for (path, content) in files {
            let hash = store.put(content.as_bytes()).unwrap();
            entries.push(ManifestEntry {
                path: path.to_string(),
                blob_hash: hash,
                size_bytes: content.len() as u64,
                content_type: Some("text/markdown".to_string()),
                updated_at: 0,
            });
        }

        let index = KnowledgeIndex::build(&entries, &store).unwrap();
        (tmp, Arc::new(RwLock::new(index)))
    }

    /// Helper: build an empty index.
    fn build_empty_index() -> (TempDir, Arc<RwLock<KnowledgeIndex>>) {
        build_index(&[])
    }

    // --- KnowledgeFreshnessEvaluator tests ---

    #[test]
    fn freshness_fresh_index_scores_high() {
        let (_tmp, index) = build_index(&[
            ("/a.md", "# A\n\nSee [[B]]."),
            ("/b.md", "# B\n\nSee [[A]]."),
        ]);
        let eval = KnowledgeFreshnessEvaluator::with_defaults(index);
        let ctx = EvalContext::new("test");

        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
        assert!(scores[0].explanation.as_ref().unwrap().contains("fresh"));
    }

    #[test]
    fn freshness_empty_index_scores_zero() {
        let (_tmp, index) = build_empty_index();
        let eval = KnowledgeFreshnessEvaluator::with_defaults(index);
        let ctx = EvalContext::new("test");

        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value).abs() < f64::EPSILON);
        assert!(scores[0].explanation.as_ref().unwrap().contains("empty"));
    }

    #[test]
    fn freshness_stale_index_scores_half() {
        let (_tmp, index) = build_index(&[("/a.md", "# A")]);
        // Zero-duration threshold makes any non-empty index immediately stale
        let eval = KnowledgeFreshnessEvaluator::new(index, Duration::ZERO);
        let ctx = EvalContext::new("test");

        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.5).abs() < f64::EPSILON);
        assert!(scores[0].explanation.as_ref().unwrap().contains("stale"));
    }

    // --- KnowledgeCoherenceEvaluator tests ---

    #[test]
    fn coherence_healthy_vault_scores_high() {
        let (_tmp, index) = build_index(&[
            ("/a.md", "# A\n\nSee [[B]]."),
            ("/b.md", "# B\n\nSee [[A]]."),
        ]);
        let eval = KnowledgeCoherenceEvaluator::new(index);
        let ctx = EvalContext::new("test");

        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
        assert!(
            scores[0]
                .explanation
                .as_ref()
                .unwrap()
                .contains("0 contradictions")
        );
    }

    #[test]
    fn coherence_broken_vault_scores_low() {
        let (_tmp, index) = build_index(&[
            ("/a.md", "# A\n\nSee [[NonExistent]]."),
            ("/b.md", "# B\n\nIsolated."),
        ]);
        let eval = KnowledgeCoherenceEvaluator::new(index);
        let ctx = EvalContext::new("test");

        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!(scores[0].value < 1.0);
    }

    // --- KnowledgeCoverageEvaluator tests ---

    #[test]
    fn coverage_full_coverage_scores_high() {
        let (_tmp, index) = build_index(&[
            ("/a.md", "# A\n\nSee [[B]]."),
            ("/b.md", "# B\n\nSee [[A]]."),
        ]);
        let eval = KnowledgeCoverageEvaluator::new(index);
        let ctx = EvalContext::new("test");

        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
        assert!(
            scores[0]
                .explanation
                .as_ref()
                .unwrap()
                .contains("All 2 referenced concepts have pages")
        );
    }

    #[test]
    fn coverage_missing_pages_scores_low() {
        let (_tmp, index) = build_index(&[
            ("/a.md", "# A\n\nSee [[X]] and [[Y]] and [[Z]]."),
            ("/b.md", "# B\n\nSee [[A]]."),
        ]);
        let eval = KnowledgeCoverageEvaluator::new(index);
        let ctx = EvalContext::new("test");

        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        // 2 notes + 3 missing = 5 total; score = 1.0 - 3/5 = 0.4
        assert!((scores[0].value - 0.4).abs() < f64::EPSILON);
        assert!(
            scores[0]
                .explanation
                .as_ref()
                .unwrap()
                .contains("missing pages")
        );
    }

    #[test]
    fn coverage_empty_index_scores_one() {
        let (_tmp, index) = build_empty_index();
        let eval = KnowledgeCoverageEvaluator::new(index);
        let ctx = EvalContext::new("test");

        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
    }
}
