//! The `BookkeepingJudge` ergon Workflow.
//!
//! `execute()` body:
//!
//! 1. Read the raw extract file from disk (tokio::fs).
//! 2. Parse `## Item N` blocks + preamble metadata (see [`crate::parse`]).
//! 3. For each item, dispatch in sequence to the three authored agents
//!    (`bookkeeping-novelty`, `bookkeeping-specificity`,
//!    `bookkeeping-relevance`) via `agent.run(&mut ctx, input)`. Each
//!    call drives the full autonomous loop via
//!    `StepCtx::run_inference_streaming` and returns the typed,
//!    schema-validated answer.
//! 4. Aggregate the three verdicts into a single [`crate::JudgedItem`]
//!    (see [`crate::score::aggregate`]).
//! 5. Emit a [`JudgeOutput`] whose JSON shape matches the bellows
//!    reference byte-for-byte.
//!
//! ## Why pre-parse instead of `fs_read`
//!
//! The bellows reference asks the model to crack the file open with
//! `fs_read` and emit one big JSON judgment. That's fine for a
//! one-shot prompt, but it loses the schema-validated per-axis I/O
//! contracts that the authored agents already encode. Pre-parsing here
//! reuses the existing agent specs verbatim — no new prompt
//! engineering, no widened tool surface.

use async_trait::async_trait;
use ergon::{
    Agent, AgentRegistry, ErgonError, FsAgentRegistry, Result as ErgonResult, Role, StepCtx,
    Workflow,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::parse::parse_extract;
use crate::score::{AxisVerdict, JudgedItem, aggregate};

/// Workflow input — same shape as the bellows reference's `JudgeInput`.
#[derive(Debug, Clone, Deserialize)]
pub struct JudgeInput {
    /// Absolute path to a raw-extract markdown file under
    /// `research/notes/`.
    pub extract_path: String,
    /// Optional hard cap on the number of items judged in one run.
    /// Defaults to 50 (matches bellows reference).
    #[serde(default)]
    pub max_items: Option<u32>,
}

impl JudgeInput {
    /// Effective max-items budget — `Some(n)` honors the caller's value,
    /// `None` falls back to 50.
    pub fn effective_max_items(&self) -> u32 {
        self.max_items.unwrap_or(50)
    }
}

/// Workflow output — same shape as the bellows reference's
/// `JudgeOutput`.
#[derive(Debug, Clone, Serialize)]
pub struct JudgeOutput {
    /// One judgment per parsed `## Item N` block.
    pub items: Vec<JudgedItem>,
    /// Echo of `JudgeInput::extract_path`.
    pub source_file: String,
    /// ISO-8601 UTC timestamp of when the workflow finished.
    pub judged_at: String,
    /// Stable provider identifier; always `ergon-authored-agents` for
    /// this port (the bellows reference returns the model provider name
    /// here, but for an ergon Workflow the provider is selected at the
    /// StepCtx level, not the workflow).
    pub provider: String,
    /// Session identifier the workflow ran under (from
    /// `StepCtx::session_id`).
    pub session_id: String,
}

/// Construction inputs the workflow needs that ergon cannot infer
/// itself: where to find the authored agent registry.
pub struct BookkeepingJudge {
    agents_dir: PathBuf,
    /// Pre-loaded registry — shared `Arc` so test setups can pre-build
    /// it once and reuse across many judge invocations.
    registry: Arc<FsAgentRegistry>,
}

impl BookkeepingJudge {
    /// Build a judge that loads agent specs from the given directory
    /// (typically `<workspace>/agents/`).
    pub fn new(agents_dir: PathBuf) -> Result<Self, ergon::RegistryError> {
        let registry = Arc::new(FsAgentRegistry::load(agents_dir.clone())?);
        Ok(Self {
            agents_dir,
            registry,
        })
    }

    /// Construct from a pre-built registry — useful when callers want
    /// to share a single registry across multiple workflow constructions
    /// (the registry is cheap to clone but expensive to compile from
    /// scratch).
    pub fn from_registry(agents_dir: PathBuf, registry: Arc<FsAgentRegistry>) -> Self {
        Self {
            agents_dir,
            registry,
        }
    }

    /// Directory the registry was loaded from.
    pub fn agents_dir(&self) -> &PathBuf {
        &self.agents_dir
    }

    /// Resolve a single agent by name. Surfaces a typed error pointing
    /// at the missing spec so a misnamed agent is a workflow-time
    /// failure, not a runtime mystery.
    async fn require_agent(&self, name: &str) -> ErgonResult<Arc<dyn Agent>> {
        match self.registry.get(name).await {
            Some(a) => Ok(a),
            None => Err(ErgonError::Workflow(format!(
                "required agent `{name}` not registered in {}",
                self.agents_dir.display()
            ))),
        }
    }
}

#[async_trait]
impl Workflow for BookkeepingJudge {
    type Input = JudgeInput;
    type Output = JudgeOutput;

    fn name(&self) -> &str {
        "bookkeeping.promotion-judge"
    }

    fn role(&self) -> Role {
        // Workflow-scope role is empty — each authored agent supplies
        // its own agent-scope role overlay, which is precisely the
        // separation the agent primitive was built for.
        Role::default()
    }

    async fn execute(&self, ctx: &mut StepCtx<'_>, input: JudgeInput) -> ErgonResult<JudgeOutput> {
        let max_items = input.effective_max_items();

        // 1. Read the raw extract.
        let body = tokio::fs::read_to_string(&input.extract_path)
            .await
            .map_err(|e| {
                ErgonError::Workflow(format!(
                    "could not read extract `{}`: {e}",
                    input.extract_path
                ))
            })?;

        // 2. Parse.
        let (meta, raw_items) = parse_extract(&body, max_items);

        if raw_items.is_empty() {
            // Empty extract: surface a clean output rather than failing
            // (matches the bellows reference's behavior — no items in,
            // no items out).
            return Ok(JudgeOutput {
                items: Vec::new(),
                source_file: input.extract_path,
                judged_at: now_iso(),
                provider: "ergon-authored-agents".to_string(),
                session_id: ctx.session_id.to_string(),
            });
        }

        // 3. Resolve the three axis agents once.
        let novelty_agent = self.require_agent("bookkeeping-novelty").await?;
        let specificity_agent = self.require_agent("bookkeeping-specificity").await?;
        let relevance_agent = self.require_agent("bookkeeping-relevance").await?;

        // 4. Dispatch each item through all three axes sequentially.
        //
        // We pre-parse on the Rust side and never advertise tools
        // beyond `record_answer` (the framework synthesizes that
        // automatically), so each agent call is a single inference
        // round with schema-validated input + output. Sequential is
        // intentional in v0.1 — concurrent dispatch would need a
        // multi-StepCtx scope swap that's outside the ergon harness
        // contract today.
        let mut items: Vec<JudgedItem> = Vec::with_capacity(raw_items.len());
        for raw in &raw_items {
            // Defensive: empty body items get a 0/0/0 verdict without
            // ever hitting the model (matches the bellows reference's
            // "give it 0/0/0 and explain" guidance).
            if raw.body.trim().is_empty() {
                items.push(JudgedItem {
                    item_number: raw.number,
                    slug: format!("item-{}", raw.number),
                    kind: "discovery".to_string(),
                    novelty: 0,
                    specificity: 0,
                    relevance: 0,
                    total: 0,
                    pass: false,
                    blog_candidate: false,
                    reasoning: "Item body is empty — no signal to score.".to_string(),
                });
                continue;
            }

            // Build per-axis inputs. Each axis agent's input schema
            // requires `item_text` + `source_type` at minimum; the
            // bookkeeping-relevance agent additionally accepts
            // `active_projects` / `open_questions` / `archived_or_paused_projects`
            // (we pass empty defaults — the schema marks them
            // optional). The bookkeeping-novelty agent accepts
            // `existing_entity_slugs` + `project_modules` (also
            // optional; default empty).

            let common_text = &raw.body;
            let novelty_input = serde_json::json!({
                "item_text": common_text,
                "source_type": meta.source_type,
                "source_url": meta.source_url,
                "existing_entity_slugs": [],
                "project_modules": [],
            });
            let specificity_input = serde_json::json!({
                "item_text": common_text,
                "source_type": meta.source_type,
                "source_url": meta.source_url,
            });
            let relevance_input = serde_json::json!({
                "item_text": common_text,
                "source_type": meta.source_type,
                "source_url": meta.source_url,
                "active_projects": [],
                "open_questions": [],
                "archived_or_paused_projects": [],
            });

            let novelty_v = novelty_agent.run(ctx, novelty_input).await?;
            let specificity_v = specificity_agent.run(ctx, specificity_input).await?;
            let relevance_v = relevance_agent.run(ctx, relevance_input).await?;

            let n_axis = AxisVerdict {
                score: extract_score(&novelty_v),
                reasoning: extract_str(&novelty_v, "reasoning"),
                closest_slug: extract_str(&novelty_v, "closest_existing_slug"),
            };
            let s_axis = AxisVerdict {
                score: extract_score(&specificity_v),
                reasoning: extract_str(&specificity_v, "reasoning"),
                closest_slug: String::new(),
            };
            let r_axis = AxisVerdict {
                score: extract_score(&relevance_v),
                reasoning: extract_str(&relevance_v, "reasoning"),
                closest_slug: String::new(),
            };

            items.push(aggregate(raw.number, n_axis, s_axis, r_axis, None, None));
        }

        Ok(JudgeOutput {
            items,
            source_file: input.extract_path,
            judged_at: now_iso(),
            provider: "ergon-authored-agents".to_string(),
            session_id: ctx.session_id.to_string(),
        })
    }
}

/// Pull the `score` field from an agent's typed answer JSON. The
/// bookkeeping-* agents declare `score: integer 0..=3`. We clamp
/// defensively because the aggregator clamps too — better to
/// converge to a saturated 3 than to bubble a panic.
fn extract_score(answer: &serde_json::Value) -> u8 {
    answer
        .get("score")
        .and_then(|v| v.as_u64())
        .map(|n| n.min(3) as u8)
        .unwrap_or(0)
}

/// Pull a string field from the agent answer; empty string if missing.
fn extract_str(answer: &serde_json::Value, key: &str) -> String {
    answer
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// RFC-3339 / ISO-8601 timestamp without a `chrono` dep. Format matches
/// what `bookkeeping.py` already writes elsewhere in the graph and what
/// the bellows reference emits.
fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let h = secs_of_day / 3_600;
    let m = (secs_of_day % 3_600) / 60;
    let s = secs_of_day % 60;
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_input_effective_max_items_defaults_to_50() {
        let input = JudgeInput {
            extract_path: "/tmp/foo".to_string(),
            max_items: None,
        };
        assert_eq!(input.effective_max_items(), 50);
    }

    #[test]
    fn judge_input_honors_explicit_max_items() {
        let input = JudgeInput {
            extract_path: "/tmp/foo".to_string(),
            max_items: Some(7),
        };
        assert_eq!(input.effective_max_items(), 7);
    }

    #[test]
    fn extract_score_clamps_and_defaults() {
        assert_eq!(extract_score(&serde_json::json!({"score": 2})), 2);
        assert_eq!(extract_score(&serde_json::json!({"score": 9})), 3);
        assert_eq!(extract_score(&serde_json::json!({"score": -1})), 0);
        assert_eq!(extract_score(&serde_json::json!({})), 0);
    }

    #[test]
    fn extract_str_defaults_to_empty() {
        assert_eq!(
            extract_str(&serde_json::json!({"reasoning": "abc"}), "reasoning"),
            "abc"
        );
        assert_eq!(extract_str(&serde_json::json!({}), "reasoning"), "");
    }

    #[test]
    fn now_iso_emits_iso_8601() {
        let ts = now_iso();
        assert_eq!(ts.len(), 20); // YYYY-MM-DDTHH:MM:SSZ
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
    }
}
