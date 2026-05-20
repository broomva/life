//! Score aggregation — combines the three per-axis agent verdicts
//! into the single [`JudgedItem`] the bellows reference emits.
//!
//! Score schema is locked to keep parity with the bellows
//! reference: `pass = total >= 5`, `blog_candidate = total >= 7`.

use serde::{Deserialize, Serialize};

/// A raw `## Item N` block extracted from the markdown file. Carries
/// the 1-indexed number and the body text the agents will judge.
#[derive(Debug, Clone)]
pub struct RawItem {
    /// 1-indexed item number from the source file's `## Item N` heading.
    pub number: u32,
    /// Free-form body — everything between this item's heading and the
    /// next heading. Trimmed.
    pub body: String,
}

/// One of the seven Layer-3 entity types the knowledge graph accepts.
/// Used to decide whether a passing item should be filed as
/// `entities/concept/`, `entities/tool/`, etc. We mirror the bellows
/// reference's enum; new types must land here and in the agent
/// instructions together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Concept,
    Pattern,
    Tool,
    Person,
    Project,
    Discovery,
    Question,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Concept => "concept",
            Self::Pattern => "pattern",
            Self::Tool => "tool",
            Self::Person => "person",
            Self::Project => "project",
            Self::Discovery => "discovery",
            Self::Question => "question",
        }
    }
}

/// The output emitted per item. Matches the bellows reference shape
/// byte-for-byte so downstream Python tooling
/// (`skills/bookkeeping/scripts/bookkeeping.py`) doesn't need to
/// change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JudgedItem {
    /// 1-indexed item number.
    pub item_number: u32,
    /// kebab-case slug; matches `existing_entity_slugs` shape from the
    /// novelty agent.
    pub slug: String,
    /// One of the [`ItemKind`] enum variants stringified
    /// (`concept`, `pattern`, `tool`, `person`, `project`,
    /// `discovery`, `question`).
    #[serde(rename = "type")]
    pub kind: String,
    /// 0..=3 (clamped — agents may return 0..=3 by schema).
    pub novelty: u8,
    /// 0..=3.
    pub specificity: u8,
    /// 0..=3.
    pub relevance: u8,
    /// `novelty + specificity + relevance` (0..=9).
    pub total: u8,
    /// `total >= 5`.
    pub pass: bool,
    /// `total >= 7`.
    pub blog_candidate: bool,
    /// One- or two-sentence aggregated reasoning. Merges the per-axis
    /// reasoning fields with the dominant axis cited first.
    pub reasoning: String,
}

/// Inputs from the three per-axis agents that the aggregator combines
/// into a single [`JudgedItem`]. Each axis carries its score, its
/// reasoning, and (for slug derivation) the novelty agent's
/// `closest_existing_slug` field.
#[derive(Debug, Clone)]
pub struct AxisVerdict {
    pub score: u8,
    pub reasoning: String,
    /// Only set on the novelty axis — empty string on the others.
    pub closest_slug: String,
}

/// Apply the bellows-shipped aggregation rules to the three per-axis
/// verdicts. Pure function — no I/O.
///
/// Caller is responsible for picking the item slug + kind. We default
/// to `"item-N"` / `"discovery"` when nothing better is available; the
/// novelty agent's `closest_existing_slug` is preferred when present.
pub fn aggregate(
    item_number: u32,
    novelty: AxisVerdict,
    specificity: AxisVerdict,
    relevance: AxisVerdict,
    explicit_slug: Option<&str>,
    explicit_kind: Option<ItemKind>,
) -> JudgedItem {
    let n = novelty.score.min(3);
    let s = specificity.score.min(3);
    let r = relevance.score.min(3);
    let total = n + s + r;
    let pass = total >= 5;
    let blog_candidate = total >= 7;

    // Slug precedence: caller-supplied → novelty.closest → fallback.
    let kind = explicit_kind.unwrap_or_else(|| infer_kind(&novelty.closest_slug));
    let closest_for_slug = novelty.closest_slug.clone();
    let slug = match explicit_slug {
        Some(x) if !x.is_empty() => x.to_string(),
        _ => {
            let fallback = format!("item-{item_number}");
            if closest_for_slug.is_empty() {
                fallback
            } else {
                // closest_slug is shaped like `concept/foo-bar`. We
                // strip the leading type prefix because the bellows
                // schema has slug separate from type.
                closest_for_slug
                    .rsplit_once('/')
                    .map(|(_, tail)| tail.to_string())
                    .unwrap_or(closest_for_slug)
            }
        }
    };

    let reasoning = format_reasoning(&novelty, &specificity, &relevance);

    JudgedItem {
        item_number,
        slug,
        kind: kind.as_str().to_string(),
        novelty: n,
        specificity: s,
        relevance: r,
        total,
        pass,
        blog_candidate,
        reasoning,
    }
}

fn infer_kind(closest_slug: &str) -> ItemKind {
    match closest_slug.split_once('/').map(|(p, _)| p) {
        Some("concept") => ItemKind::Concept,
        Some("pattern") => ItemKind::Pattern,
        Some("tool") => ItemKind::Tool,
        Some("person") => ItemKind::Person,
        Some("project") => ItemKind::Project,
        Some("discovery") => ItemKind::Discovery,
        Some("question") => ItemKind::Question,
        _ => ItemKind::Discovery,
    }
}

/// Compose a single 1-2 sentence justification from the three per-axis
/// reasonings. Tagged with `[axis score]` prefixes so reviewers can
/// scan the load-bearing signal without re-parsing the prose.
fn format_reasoning(
    novelty: &AxisVerdict,
    specificity: &AxisVerdict,
    relevance: &AxisVerdict,
) -> String {
    // Order is fixed (relevance → novelty → specificity) because
    // relevance is the most strategically load-bearing in the Nous
    // gate: a high-relevance / low-novelty item still earns Layer-3
    // status, but the converse rarely does.
    let triples = [
        ("relevance", relevance.score, &relevance.reasoning),
        ("novelty", novelty.score, &novelty.reasoning),
        ("specificity", specificity.score, &specificity.reasoning),
    ];

    let mut buf = String::new();
    for (label, score, reasoning) in &triples {
        if !buf.is_empty() {
            buf.push(' ');
        }
        // Truncate per-axis reasoning to keep the combined string
        // bounded; the bellows reference asks the model itself for
        // terse reasoning, but axis-summing can balloon if each
        // returns 200 chars.
        let truncated = truncate(reasoning, 220);
        buf.push_str(&format!("[{label} {score}] {truncated}"));
    }
    buf
}

fn truncate(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let mut end = 0;
    for (i, _) in s.char_indices().take(max_chars) {
        end = i + s[i..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn av(score: u8, reasoning: &str, closest: &str) -> AxisVerdict {
        AxisVerdict {
            score,
            reasoning: reasoning.to_string(),
            closest_slug: closest.to_string(),
        }
    }

    #[test]
    fn passing_aggregate_total_pass_blog_candidate() {
        // 3 + 3 + 1 = 7 → pass + blog_candidate.
        let item = aggregate(
            1,
            av(3, "introduces ABC", "concept/abc"),
            av(3, "named numbers", ""),
            av(1, "tangential", ""),
            None,
            None,
        );
        assert_eq!(item.novelty, 3);
        assert_eq!(item.specificity, 3);
        assert_eq!(item.relevance, 1);
        assert_eq!(item.total, 7);
        assert!(item.pass);
        assert!(item.blog_candidate);
        assert_eq!(item.kind, "concept");
        assert_eq!(item.slug, "abc"); // stripped `concept/` prefix
    }

    #[test]
    fn just_above_passing_threshold() {
        // 2 + 2 + 1 = 5 → pass, NOT blog_candidate.
        let item = aggregate(
            2,
            av(2, "extends", ""),
            av(2, "ok", ""),
            av(1, "ok", ""),
            None,
            None,
        );
        assert_eq!(item.total, 5);
        assert!(item.pass);
        assert!(!item.blog_candidate);
    }

    #[test]
    fn below_passing_threshold() {
        // 1 + 1 + 1 = 3 → fail.
        let item = aggregate(3, av(1, "", ""), av(1, "", ""), av(1, "", ""), None, None);
        assert_eq!(item.total, 3);
        assert!(!item.pass);
        assert!(!item.blog_candidate);
    }

    #[test]
    fn clamps_out_of_range_scores() {
        // Defensive: even if the axis emits a 9, we clamp to 3.
        let item = aggregate(4, av(9, "", ""), av(9, "", ""), av(9, "", ""), None, None);
        assert_eq!(item.novelty, 3);
        assert_eq!(item.specificity, 3);
        assert_eq!(item.relevance, 3);
        assert_eq!(item.total, 9);
    }

    #[test]
    fn explicit_slug_and_kind_override_inference() {
        let item = aggregate(
            5,
            av(1, "", "concept/foo"),
            av(1, "", ""),
            av(1, "", ""),
            Some("custom-slug"),
            Some(ItemKind::Tool),
        );
        assert_eq!(item.slug, "custom-slug");
        assert_eq!(item.kind, "tool");
    }

    #[test]
    fn fallback_slug_when_no_closest_or_explicit() {
        let item = aggregate(7, av(3, "", ""), av(1, "", ""), av(1, "", ""), None, None);
        assert_eq!(item.slug, "item-7");
    }

    #[test]
    fn reasoning_cites_all_three_axes() {
        let item = aggregate(
            1,
            av(2, "novelty-text", ""),
            av(3, "specificity-text", ""),
            av(1, "relevance-text", ""),
            None,
            None,
        );
        assert!(item.reasoning.contains("novelty-text"));
        assert!(item.reasoning.contains("specificity-text"));
        assert!(item.reasoning.contains("relevance-text"));
        assert!(item.reasoning.contains("[novelty 2]"));
        assert!(item.reasoning.contains("[specificity 3]"));
        assert!(item.reasoning.contains("[relevance 1]"));
    }
}
