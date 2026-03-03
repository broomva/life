//! SKILL.md frontmatter parser.
//!
//! Parses YAML frontmatter from SKILL.md files into [`SkillMetadata`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Parsed SKILL.md frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub user_invocable: Option<bool>,
    #[serde(default)]
    pub disable_model_invocation: Option<bool>,
    /// Arbitrary key-value metadata.
    #[serde(default, flatten)]
    pub metadata: BTreeMap<String, serde_yaml::Value>,
}

/// Parse a SKILL.md file into metadata + body.
pub fn parse_skill_md(content: &str) -> Result<(SkillMetadata, String), SkillParseError> {
    let trimmed = content.trim();

    if !trimmed.starts_with("---") {
        return Err(SkillParseError::MissingFrontmatter);
    }

    // Find the closing "---" (skip the first one)
    let after_first = &trimmed[3..];
    let closing = after_first
        .find("\n---")
        .ok_or(SkillParseError::MissingFrontmatter)?;

    let yaml_str = after_first[..closing].trim();
    let body_start = 3 + closing + 4; // skip "\n---"
    let body = if body_start < trimmed.len() {
        trimmed[body_start..].trim().to_string()
    } else {
        String::new()
    };

    let meta: SkillMetadata =
        serde_yaml::from_str(yaml_str).map_err(|e| SkillParseError::YamlParse(e.to_string()))?;

    if meta.name.is_empty() {
        return Err(SkillParseError::MissingField("name".into()));
    }
    if meta.description.is_empty() {
        return Err(SkillParseError::MissingField("description".into()));
    }

    Ok((meta, body))
}

/// Errors from SKILL.md parsing.
#[derive(Debug, Error)]
pub enum SkillParseError {
    #[error("SKILL.md missing YAML frontmatter (must start and end with ---)")]
    MissingFrontmatter,
    #[error("YAML parse error: {0}")]
    YamlParse(String),
    #[error("missing required field: {0}")]
    MissingField(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_skill_md() {
        let content = r#"---
name: commit-helper
description: Helps create well-structured git commits
license: MIT
tags:
  - git
  - workflow
allowed_tools:
  - bash
  - read_file
user_invocable: true
---
# Commit Helper

When the user asks to commit, follow these steps:
1. Run `git status` to see changes
2. Draft a commit message
3. Ask for confirmation
"#;

        let (meta, body) = parse_skill_md(content).unwrap();
        assert_eq!(meta.name, "commit-helper");
        assert_eq!(meta.description, "Helps create well-structured git commits");
        assert_eq!(meta.license, Some("MIT".to_string()));
        assert_eq!(meta.tags, vec!["git", "workflow"]);
        assert_eq!(
            meta.allowed_tools,
            Some(vec!["bash".to_string(), "read_file".to_string()])
        );
        assert_eq!(meta.user_invocable, Some(true));
        assert!(body.contains("# Commit Helper"));
        assert!(body.contains("Run `git status`"));
    }

    #[test]
    fn parse_minimal_skill_md() {
        let content = r#"---
name: simple
description: A simple skill
---
Just do the thing.
"#;
        let (meta, body) = parse_skill_md(content).unwrap();
        assert_eq!(meta.name, "simple");
        assert_eq!(meta.description, "A simple skill");
        assert_eq!(meta.tags, Vec::<String>::new());
        assert_eq!(meta.allowed_tools, None);
        assert_eq!(meta.user_invocable, None);
        assert_eq!(body, "Just do the thing.");
    }

    #[test]
    fn parse_missing_frontmatter_fails() {
        let content = "# No frontmatter\nJust text.";
        assert!(parse_skill_md(content).is_err());
    }

    #[test]
    fn parse_missing_name_fails() {
        let content = r#"---
description: No name field
---
Body."#;
        assert!(parse_skill_md(content).is_err());
    }

    #[test]
    fn parse_empty_name_fails() {
        let content = r#"---
name: ""
description: Empty name
---
Body."#;
        let err = parse_skill_md(content).unwrap_err();
        assert!(err.to_string().contains("name"));
    }
}
