//! Markdown extract parser.
//!
//! Raw extracts under `research/notes/*-raw.md` follow a stable shape:
//!
//! ```markdown
//! # Title
//!
//! Source URL: ...
//! Source type: x-thread
//! ...
//!
//! ## Item 1
//!
//! <free-form body>
//!
//! ## Item 2
//!
//! <free-form body>
//! ```
//!
//! We extract every `## Item N` block plus the file-level metadata
//! (source URL, source type) that the per-axis agents need as input.
//!
//! The bellows reference asks the model to crack the file open itself
//! with `fs_read`, then to produce one JSON judgment per item. Here we
//! pre-parse on the Rust side and dispatch one agent call per (item,
//! axis) — schema-validated input + schema-validated output at every
//! boundary. The agent never sees a raw markdown blob.

use crate::score::RawItem;

/// File-level metadata extracted from the extract preamble.
#[derive(Debug, Clone)]
pub struct ExtractMeta {
    /// Source-type tag, e.g. `x-thread`, `moltbook`, `conversation`.
    /// Bookkeeping agents enumerate this — see
    /// `agents/bookkeeping-{novelty,specificity,relevance}.md`. Falls
    /// back to `internal-doc` if absent so the agents always have a
    /// valid enum value.
    pub source_type: String,
    /// Canonical URL of the source; empty string if not declared in the
    /// preamble.
    pub source_url: String,
}

impl ExtractMeta {
    /// Construct with the default source type for unlabeled files.
    pub fn default_for(_path: &str) -> Self {
        Self {
            source_type: "internal-doc".to_string(),
            source_url: String::new(),
        }
    }
}

/// Parse the extract markdown body into the file-level metadata and
/// the list of `## Item N` blocks. Items past `max_items` are skipped
/// (matches the bellows reference's hard-cap behavior).
pub fn parse_extract(body: &str, max_items: u32) -> (ExtractMeta, Vec<RawItem>) {
    let mut meta = ExtractMeta {
        source_type: String::new(),
        source_url: String::new(),
    };

    // ── Pass 1 — preamble metadata before the first `## Item` heading.
    for line in body.lines() {
        if line.trim_start().starts_with("## Item ") {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("source url:") {
            meta.source_url = rest.trim().to_string();
        } else if let Some(rest) = lower.strip_prefix("- source url:") {
            meta.source_url = rest.trim().to_string();
        } else if let Some(rest) = lower.strip_prefix("source type:") {
            meta.source_type = rest.trim().to_string();
        } else if let Some(rest) = lower.strip_prefix("- source type:") {
            meta.source_type = rest.trim().to_string();
        }
    }

    if meta.source_type.is_empty() {
        meta.source_type = "internal-doc".to_string();
    }

    // ── Pass 2 — collect items. `## Item <N>` starts a block; everything
    // up to the next `## ` heading is the body.
    let mut items: Vec<RawItem> = Vec::new();
    let mut current: Option<(u32, Vec<String>)> = None;

    for line in body.lines() {
        if let Some(num) = parse_item_heading(line) {
            if let Some((n, lines)) = current.take() {
                items.push(RawItem {
                    number: n,
                    body: lines.join("\n").trim().to_string(),
                });
            }
            current = Some((num, Vec::new()));
            continue;
        }
        // Any new `## ` heading that is NOT an item heading closes the
        // current item.
        if line.trim_start().starts_with("## ") && current.is_some() {
            if let Some((n, lines)) = current.take() {
                items.push(RawItem {
                    number: n,
                    body: lines.join("\n").trim().to_string(),
                });
            }
            continue;
        }
        if let Some((_, ref mut lines)) = current {
            lines.push(line.to_string());
        }
    }
    if let Some((n, lines)) = current {
        items.push(RawItem {
            number: n,
            body: lines.join("\n").trim().to_string(),
        });
    }

    // Hard cap (matches bellows reference).
    let cap = max_items.max(1) as usize;
    items.truncate(cap);

    (meta, items)
}

/// Parse a markdown heading of the form `## Item <N>` (case-insensitive
/// on "Item"). Returns the parsed number, or `None` if the line is
/// not an item heading.
fn parse_item_heading(line: &str) -> Option<u32> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("## ")?;
    let rest = rest.trim();
    // `Item 12 — description` → keep first word "Item", then the number.
    let mut parts = rest.splitn(2, char::is_whitespace);
    let head = parts.next()?;
    if !head.eq_ignore_ascii_case("item") {
        return None;
    }
    let rest = parts.next()?.trim_start();
    // Take the leading numeric run.
    let mut digits = String::new();
    for c in rest.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Sample\n\nSource URL: https://example.com\nSource type: x-thread\n\n## Item 1\n\nFirst item body line A.\nLine B.\n\n## Item 2\n\nSecond item body.\n\n## Notes\n\nfooter — should NOT become an item.\n";

    #[test]
    fn parses_preamble_metadata() {
        let (meta, _items) = parse_extract(SAMPLE, 10);
        assert_eq!(meta.source_url, "https://example.com");
        assert_eq!(meta.source_type, "x-thread");
    }

    #[test]
    fn parses_two_items_stopping_at_unrelated_heading() {
        let (_, items) = parse_extract(SAMPLE, 10);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].number, 1);
        assert!(items[0].body.contains("First item body line A."));
        assert!(items[0].body.contains("Line B."));
        assert_eq!(items[1].number, 2);
        assert!(items[1].body.contains("Second item body."));
    }

    #[test]
    fn max_items_caps_the_list() {
        let (_, items) = parse_extract(SAMPLE, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 1);
    }

    #[test]
    fn defaults_source_type_when_absent() {
        let body = "# Title\n\n## Item 1\n\nbody\n";
        let (meta, _) = parse_extract(body, 5);
        assert_eq!(meta.source_type, "internal-doc");
        assert!(meta.source_url.is_empty());
    }

    #[test]
    fn item_heading_with_dash_description_still_parses() {
        let body = "## Item 7 — interesting fact\n\nbody\n";
        let (_, items) = parse_extract(body, 5);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 7);
    }
}
