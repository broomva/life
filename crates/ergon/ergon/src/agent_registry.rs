//! Agent registry — name-keyed lookup of [`crate::Agent`] instances.
//!
//! The registry is the substrate that makes authored AgentSpecs
//! invocable by name. Per architecture spec
//! `docs/superpowers/specs/2026-05-09-bro-1006-authored-agents-architecture.md`
//! §3, this is Layer 2 — substrate plumbing that hosts authored
//! patterns (Layer 3 data).
//!
//! ## Three implementations
//!
//! 1. [`InMemoryAgentRegistry`] — for tests, programmatically-built
//!    registries, and binaries that ship a fixed set of agents.
//! 2. [`FsAgentRegistry`] — loads `agents/<name>.md` files from a
//!    directory tree. Files are Markdown with YAML frontmatter; the
//!    body becomes [`crate::AgentSpec::instructions`].
//! 3. (Deferred) `LagoAgentRegistry` — reads `Custom("agent.spec")`
//!    events. Lives in a sibling crate (out of scope for this PR;
//!    defer until lago dep is appropriate).
//!
//! ## Authoring format
//!
//! `agents/<name>.md`:
//!
//! ```markdown
//! ---
//! name: bookkeeping.score-extract
//! model: claude-haiku-4-5
//! max_turns: 1
//! max_retries: 3
//! allowed_tools: []
//! input_schema:
//!   type: object
//!   properties:
//!     text: { type: string }
//!   required: [text]
//! output_schema:
//!   type: object
//!   properties:
//!     score: { type: integer, minimum: 0, maximum: 9 }
//!   required: [score]
//! ---
//!
//! # Score the extract
//!
//! You score raw research extracts on a 0-9 scale...
//! ```
//!
//! See spec §4 for the rationale.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::agent::{Agent, AgentSpec};
use crate::error::ErgonError;

/// Trait the substrate uses to look up agents by name.
///
/// Multiple implementations coexist: [`InMemoryAgentRegistry`] for
/// programmatic / test use, [`FsAgentRegistry`] for `agents/*.md`
/// files. A future `LagoAgentRegistry` (sibling crate) reads from
/// the lago event journal for the experimental tier.
///
/// Hosts can compose multiple registries via [`ChainedAgentRegistry`]:
/// authored MD files take precedence over experimental lago entries,
/// or vice versa, depending on the deployment's policy.
#[async_trait]
pub trait AgentRegistry: Send + Sync {
    /// Resolve an agent by name. Returns `None` if no agent is
    /// registered under the given name.
    async fn get(&self, name: &str) -> Option<Arc<dyn Agent>>;

    /// All registered agent names. Used for diagnostics and the
    /// `arcan agent list` CLI. Order is implementation-defined.
    async fn names(&self) -> Vec<String>;

    /// Number of registered agents.
    async fn len(&self) -> usize {
        self.names().await.len()
    }

    /// True iff the registry has zero agents.
    async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

// ─── InMemoryAgentRegistry ──────────────────────────────────────────────

/// Programmatic registry — agents registered at construction time.
/// Suitable for tests and binaries that ship with a fixed set.
pub struct InMemoryAgentRegistry {
    entries: RwLock<HashMap<String, Arc<dyn Agent>>>,
}

impl std::fmt::Debug for InMemoryAgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<String> = self
            .entries
            .read()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        f.debug_struct("InMemoryAgentRegistry")
            .field("agents", &names)
            .finish()
    }
}

impl InMemoryAgentRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an agent. Panics on duplicate names — the registry is
    /// typically populated once at startup, and silent overwrites
    /// would mask configuration bugs.
    pub fn insert(&self, agent: Arc<dyn Agent>) {
        let name = agent.spec().name;
        let mut entries = self.entries.write().expect("poisoned");
        if entries.contains_key(&name) {
            panic!("agent `{name}` already registered");
        }
        entries.insert(name, agent);
    }

    /// Builder-style insert (returns `self` so chains work).
    #[must_use]
    pub fn with(self, agent: Arc<dyn Agent>) -> Self {
        self.insert(agent);
        self
    }

    /// Insert a [`crate::AgentSpec`] directly. Wraps the spec in an
    /// `Arc<dyn Agent>` (since `AgentSpec` impls `Agent`) and
    /// registers it under its `name`.
    pub fn insert_spec(&self, spec: AgentSpec) {
        self.insert(Arc::new(spec));
    }
}

impl Default for InMemoryAgentRegistry {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl AgentRegistry for InMemoryAgentRegistry {
    async fn get(&self, name: &str) -> Option<Arc<dyn Agent>> {
        self.entries.read().expect("poisoned").get(name).cloned()
    }

    async fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .entries
            .read()
            .expect("poisoned")
            .keys()
            .cloned()
            .collect();
        v.sort();
        v
    }
}

// ─── FsAgentRegistry ────────────────────────────────────────────────────

/// Loads agents from `agents/*.md` files (recursively) at the
/// supplied root.
///
/// Uses [`parse_agent_md`] to convert Markdown-with-frontmatter into
/// [`AgentSpec`]. The agent's filename (without `.md` extension) MUST
/// match the `name` field in frontmatter — fail-fast on mismatch
/// keeps the registry name-canonical.
///
/// Re-load via [`FsAgentRegistry::reload`] when files change.
pub struct FsAgentRegistry {
    root: PathBuf,
    inner: InMemoryAgentRegistry,
}

impl std::fmt::Debug for FsAgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsAgentRegistry")
            .field("root", &self.root)
            .field("inner", &self.inner)
            .finish()
    }
}

impl FsAgentRegistry {
    /// Construct from a root directory and load all `*.md` files
    /// found within (recursively). Errors during load are collected
    /// and returned together — partial loads are NOT applied (we
    /// either load the full registry cleanly or return the errors).
    pub fn load(root: impl AsRef<Path>) -> Result<Self, RegistryError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_dir() {
            return Err(RegistryError::RootNotADir(root));
        }

        let mut errors: Vec<RegistryError> = Vec::new();
        let mut specs: Vec<(String, AgentSpec)> = Vec::new();

        for entry in walk_md_files(&root) {
            match entry {
                Ok(path) => {
                    match std::fs::read_to_string(&path) {
                        Ok(content) => match parse_agent_md(&content) {
                            Ok(spec) => {
                                // Filename / name match check.
                                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                                if stem != spec.name {
                                    errors.push(RegistryError::NameMismatch {
                                        path: path.clone(),
                                        filename_stem: stem.to_owned(),
                                        spec_name: spec.name.clone(),
                                    });
                                    continue;
                                }
                                specs.push((spec.name.clone(), spec));
                            }
                            // Files without YAML frontmatter are
                            // documentation (e.g. `agents/README.md`,
                            // `CHANGELOG.md`), not agents — skip them
                            // silently rather than failing the whole
                            // registry load. Same goes for empty-body
                            // files: a stub README without a real body
                            // shouldn't take down the kernel boot.
                            // Genuine agent files have BOTH frontmatter
                            // AND a body, and any other parse error
                            // (malformed YAML, unknown field, …) is
                            // still a hard error so typos in real
                            // agents still fail loud.
                            Err(ParseError::MissingFrontmatter | ParseError::EmptyBody) => {
                                tracing::debug!(
                                    target: "ergon.agent_registry",
                                    path = %path.display(),
                                    "skipping non-agent markdown file (no frontmatter or empty body)",
                                );
                            }
                            Err(e) => errors.push(RegistryError::Parse {
                                path: path.clone(),
                                source: Box::new(e),
                            }),
                        },
                        Err(e) => errors.push(RegistryError::Io {
                            path: path.clone(),
                            source: e,
                        }),
                    }
                }
                Err(e) => errors.push(e),
            }
        }

        if !errors.is_empty() {
            return Err(RegistryError::AggregateLoad { root, errors });
        }

        let mem = InMemoryAgentRegistry::new();
        for (_, spec) in specs {
            mem.insert_spec(spec);
        }
        Ok(Self { root, inner: mem })
    }

    /// Re-load all `*.md` files from the same root. Replaces the
    /// in-memory registry atomically on success.
    pub fn reload(&mut self) -> Result<(), RegistryError> {
        let fresh = Self::load(&self.root)?;
        self.inner = fresh.inner;
        Ok(())
    }

    /// Path the registry was loaded from.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl AgentRegistry for FsAgentRegistry {
    async fn get(&self, name: &str) -> Option<Arc<dyn Agent>> {
        self.inner.get(name).await
    }
    async fn names(&self) -> Vec<String> {
        self.inner.names().await
    }
}

// ─── ChainedAgentRegistry ───────────────────────────────────────────────

/// Compose multiple [`AgentRegistry`] sources. `get()` consults each
/// in order, returning the first hit. `names()` returns the union.
///
/// Typical use: filesystem (blessed) + lago (experimental). The
/// blessed tier shadows experimental for stability.
pub struct ChainedAgentRegistry {
    sources: Vec<Arc<dyn AgentRegistry>>,
}

impl ChainedAgentRegistry {
    pub fn new(sources: Vec<Arc<dyn AgentRegistry>>) -> Self {
        Self { sources }
    }
}

#[async_trait]
impl AgentRegistry for ChainedAgentRegistry {
    async fn get(&self, name: &str) -> Option<Arc<dyn Agent>> {
        for src in &self.sources {
            if let Some(a) = src.get(name).await {
                return Some(a);
            }
        }
        None
    }
    async fn names(&self) -> Vec<String> {
        let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for src in &self.sources {
            for n in src.names().await {
                out.insert(n);
            }
        }
        out.into_iter().collect()
    }
}

// ─── parse_agent_md ─────────────────────────────────────────────────────

/// Frontmatter shape for `agents/<name>.md` files. This is the
/// authored projection of [`AgentSpec`] — `instructions` lives in the
/// Markdown body, not in frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentFrontmatter {
    pub name: String,
    pub model: String,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_max_retries")]
    pub max_retries: u8,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
}

fn default_max_turns() -> u32 {
    16
}
fn default_max_retries() -> u8 {
    3
}

/// Parse a Markdown-with-frontmatter document into an [`AgentSpec`].
///
/// The frontmatter is YAML carrying the structured fields; the body
/// becomes the `instructions` field (whitespace-trimmed). Errors
/// surface as [`ParseError`] variants pointing at the offending part.
pub fn parse_agent_md(content: &str) -> Result<AgentSpec, ParseError> {
    use gray_matter::{Matter, engine::YAML};

    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(content);

    // gray_matter returns Pod (its own typed value). Re-serialize to
    // YAML and parse with serde_yaml — straightforward and
    // schema-validated.
    let frontmatter = parsed.data.ok_or(ParseError::MissingFrontmatter)?;

    // gray_matter's Pod doesn't implement Serialize directly in 0.2,
    // so we go through its `as_*` accessors via serde_yaml::to_value.
    let yaml_str = pod_to_yaml_string(&frontmatter)?;
    let fm: AgentFrontmatter = serde_yaml::from_str(&yaml_str)
        .map_err(|e| ParseError::FrontmatterDeserialize(e.to_string()))?;

    let body = parsed.content.trim().to_string();
    if body.is_empty() {
        return Err(ParseError::EmptyBody);
    }

    let spec = AgentSpec::new(fm.name, fm.model, body, fm.input_schema, fm.output_schema)
        .with_max_turns(fm.max_turns)
        .with_max_retries(fm.max_retries);

    let spec = match fm.allowed_tools {
        Some(tools) => spec.with_allowed_tools(tools),
        None => spec,
    };

    let mut spec = spec;
    for (k, v) in fm.extensions {
        spec = spec.with_extension(k, v);
    }

    Ok(spec)
}

/// Render the gray_matter `Pod` back to a YAML string we can parse
/// with serde_yaml. Uses gray_matter's `serde::Serialize` impl in
/// 0.2 via a generic deserialize pattern — gray_matter v0.2 produces
/// a `Pod` that converts to JSON via `Pod::deserialize`. We go
/// through JSON since that round-trips cleanly to/from YAML.
fn pod_to_yaml_string(pod: &gray_matter::Pod) -> Result<String, ParseError> {
    // gray_matter::Pod -> serde_json::Value via its Deserialize impl.
    let json: serde_json::Value = pod
        .clone()
        .deserialize()
        .map_err(|e| ParseError::FrontmatterDeserialize(e.to_string()))?;
    serde_yaml::to_string(&json).map_err(|e| ParseError::FrontmatterDeserialize(e.to_string()))
}

/// Errors from [`parse_agent_md`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ParseError {
    #[error("agent file is missing YAML frontmatter (no `---` block at top)")]
    MissingFrontmatter,
    #[error("agent file's body (instructions) is empty")]
    EmptyBody,
    #[error("frontmatter failed deserialization: {0}")]
    FrontmatterDeserialize(String),
}

impl From<ParseError> for ErgonError {
    fn from(value: ParseError) -> Self {
        ErgonError::internal(value.to_string())
    }
}

// ─── RegistryError ──────────────────────────────────────────────────────

/// Failures that can occur during registry construction or lookup.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RegistryError {
    #[error("registry root `{0:?}` is not a directory")]
    RootNotADir(PathBuf),
    #[error("io error reading agent file `{path:?}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in agent file `{path:?}`: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<ParseError>,
    },
    #[error(
        "filename / spec name mismatch in `{path:?}`: filename stem is `{filename_stem}` but \
         spec.name is `{spec_name}` (the registry requires they match)"
    )]
    NameMismatch {
        path: PathBuf,
        filename_stem: String,
        spec_name: String,
    },
    #[error("walkdir error: {0}")]
    Walk(String),
    #[error(
        "{} error(s) while loading agents from `{root:?}` (first: {})",
        errors.len(),
        errors.first().map(ToString::to_string).unwrap_or_default()
    )]
    AggregateLoad {
        root: PathBuf,
        errors: Vec<RegistryError>,
    },
}

// ─── walk helper (without pulling walkdir as a dep — std fs is enough) ──

fn walk_md_files(root: &Path) -> Vec<Result<PathBuf, RegistryError>> {
    let mut out = Vec::new();
    walk_md_files_inner(root, &mut out);
    out
}

fn walk_md_files_inner(dir: &Path, out: &mut Vec<Result<PathBuf, RegistryError>>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            out.push(Err(RegistryError::Io {
                path: dir.to_path_buf(),
                source: e,
            }));
            return;
        }
    };
    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                out.push(Err(RegistryError::Walk(e.to_string())));
                continue;
            }
        };
        let path = entry.path();
        if path.is_dir() {
            walk_md_files_inner(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(Ok(path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── parse_agent_md ──

    #[test]
    fn parse_agent_md_happy_path() {
        let md = r#"---
name: demo.scorer
model: claude-haiku-4-5
max_turns: 2
max_retries: 1
input_schema:
  type: object
  properties:
    text: { type: string }
  required: [text]
output_schema:
  type: object
  properties:
    score: { type: integer }
  required: [score]
---

# Score the input

You score the input on a scale of 0-9.
"#;
        let spec = parse_agent_md(md).expect("parse ok");
        assert_eq!(spec.name, "demo.scorer");
        assert_eq!(spec.model, "claude-haiku-4-5");
        assert_eq!(spec.max_turns, 2);
        assert_eq!(spec.max_retries, 1);
        assert!(spec.instructions.contains("Score the input"));
        assert!(spec.input_schema.is_object());
        assert!(spec.output_schema.is_object());
    }

    #[test]
    fn parse_agent_md_with_allowed_tools() {
        let md = r#"---
name: demo.with-tools
model: claude-haiku-4-5
allowed_tools: [read_file, web_search]
input_schema:
  type: object
  properties: {}
output_schema:
  type: object
  properties: {}
---

# Demo
Body.
"#;
        let spec = parse_agent_md(md).expect("parse ok");
        assert_eq!(
            spec.allowed_tools,
            Some(vec!["read_file".to_string(), "web_search".to_string()])
        );
    }

    #[test]
    fn parse_agent_md_with_extensions() {
        let md = r#"---
name: demo.with-ext
model: m
input_schema: {type: object, properties: {}}
output_schema: {type: object, properties: {}}
extensions:
  backend_hint: mlx
  custom_field: { foo: 1 }
---

# Demo
"#;
        let spec = parse_agent_md(md).expect("parse ok");
        assert_eq!(
            spec.extensions.get("backend_hint").and_then(|v| v.as_str()),
            Some("mlx")
        );
        assert!(spec.extensions.contains_key("custom_field"));
    }

    #[test]
    fn parse_agent_md_rejects_missing_frontmatter() {
        let md = "# No frontmatter\n\nJust a body.";
        let err = parse_agent_md(md).expect_err("must reject");
        assert!(matches!(err, ParseError::MissingFrontmatter));
    }

    #[test]
    fn parse_agent_md_rejects_empty_body() {
        let md = r#"---
name: demo.empty
model: m
input_schema: {type: object}
output_schema: {type: object}
---

"#;
        let err = parse_agent_md(md).expect_err("must reject empty body");
        assert!(matches!(err, ParseError::EmptyBody));
    }

    #[test]
    fn parse_agent_md_rejects_missing_required_field() {
        let md = r#"---
name: demo.bad
input_schema: {type: object}
output_schema: {type: object}
---

# Body
"#;
        // Missing `model` field — frontmatter deserialize fails.
        let err = parse_agent_md(md).expect_err("must reject missing model");
        assert!(matches!(err, ParseError::FrontmatterDeserialize(_)));
    }

    // ── InMemoryAgentRegistry ──

    #[tokio::test]
    async fn in_memory_registry_get_and_names() {
        let reg = InMemoryAgentRegistry::new();
        let spec_a = AgentSpec::new(
            "a",
            "m",
            "Body A",
            serde_json::json!({"type": "object"}),
            serde_json::json!({"type": "object"}),
        );
        let spec_b = AgentSpec::new(
            "b",
            "m",
            "Body B",
            serde_json::json!({"type": "object"}),
            serde_json::json!({"type": "object"}),
        );
        reg.insert_spec(spec_a);
        reg.insert_spec(spec_b);

        assert_eq!(reg.len().await, 2);
        let mut names = reg.names().await;
        names.sort();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);

        assert!(reg.get("a").await.is_some());
        assert!(reg.get("b").await.is_some());
        assert!(reg.get("missing").await.is_none());
    }

    #[tokio::test]
    #[should_panic(expected = "agent `dup` already registered")]
    async fn in_memory_registry_panics_on_duplicate() {
        let reg = InMemoryAgentRegistry::new();
        let spec = AgentSpec::new(
            "dup",
            "m",
            "x",
            serde_json::json!({"type": "object"}),
            serde_json::json!({"type": "object"}),
        );
        reg.insert_spec(spec.clone());
        reg.insert_spec(spec);
    }

    // ── FsAgentRegistry ──

    fn write_agent(dir: &Path, filename: &str, content: &str) {
        std::fs::write(dir.join(filename), content).expect("write");
    }

    fn make_temp_dir(name: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ergon-fs-registry-{name}-{suffix}-{counter}"));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn valid_agent_md(name: &str) -> String {
        format!(
            r#"---
name: {name}
model: claude-haiku-4-5
input_schema:
  type: object
  properties: {{}}
output_schema:
  type: object
  properties: {{}}
---

# {name}

Body.
"#
        )
    }

    #[tokio::test]
    async fn fs_registry_loads_md_files() {
        let dir = make_temp_dir("happy");
        write_agent(&dir, "alpha.md", &valid_agent_md("alpha"));
        write_agent(&dir, "beta.md", &valid_agent_md("beta"));

        let reg = FsAgentRegistry::load(&dir).expect("load ok");
        assert_eq!(reg.len().await, 2);
        let mut names = reg.names().await;
        names.sort();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[tokio::test]
    async fn fs_registry_recurses_into_subdirs() {
        let dir = make_temp_dir("recurse");
        let sub = dir.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        write_agent(&dir, "top.md", &valid_agent_md("top"));
        write_agent(&sub, "deep.md", &valid_agent_md("deep"));

        let reg = FsAgentRegistry::load(&dir).expect("load ok");
        assert_eq!(reg.len().await, 2);
        assert!(reg.get("top").await.is_some());
        assert!(reg.get("deep").await.is_some());
    }

    #[test]
    fn fs_registry_rejects_filename_name_mismatch() {
        let dir = make_temp_dir("mismatch");
        write_agent(&dir, "actual-name.md", &valid_agent_md("declared-name"));

        let err = FsAgentRegistry::load(&dir).expect_err("must reject mismatch");
        if let RegistryError::AggregateLoad { errors, .. } = err {
            assert!(matches!(
                errors.first(),
                Some(RegistryError::NameMismatch { .. })
            ));
        } else {
            panic!("expected AggregateLoad with NameMismatch inside");
        }
    }

    #[tokio::test]
    async fn fs_registry_skips_files_without_frontmatter() {
        // Files without YAML frontmatter are documentation
        // (e.g. README.md, CHANGELOG.md), not agents. The registry
        // should silently skip them rather than failing the entire
        // load — otherwise any project that ships an agents/ folder
        // can't also ship docs alongside.
        let dir = make_temp_dir("docs");
        write_agent(&dir, "README.md", "# Just docs\n\nNo frontmatter.");
        write_agent(&dir, "real-agent.md", &valid_agent_md("real-agent"));

        let reg = FsAgentRegistry::load(&dir).expect("load skips README, accepts real agent");
        assert_eq!(reg.len().await, 1);
        assert!(reg.get("real-agent").await.is_some());
    }

    #[test]
    fn fs_registry_rejects_malformed_frontmatter() {
        // Genuine errors in frontmatter (broken YAML, unknown field)
        // are still hard failures — typos in real agents must fail
        // loud at load time, not silently.
        let dir = make_temp_dir("malformed");
        write_agent(
            &dir,
            "broken.md",
            "---\nname: broken\n  bad: indentation\n---\n\n# Body\n",
        );

        let err = FsAgentRegistry::load(&dir).expect_err("must reject");
        if let RegistryError::AggregateLoad { errors, .. } = err {
            assert!(matches!(errors.first(), Some(RegistryError::Parse { .. })));
        } else {
            panic!("expected AggregateLoad");
        }
    }

    #[test]
    fn fs_registry_rejects_non_directory_root() {
        let err = FsAgentRegistry::load("/this/does/not/exist/probably").expect_err("must reject");
        assert!(matches!(err, RegistryError::RootNotADir(_)));
    }

    // ── ChainedAgentRegistry ──

    #[tokio::test]
    async fn chained_registry_first_hit_wins() {
        let primary = Arc::new(InMemoryAgentRegistry::new());
        let secondary = Arc::new(InMemoryAgentRegistry::new());
        primary.insert_spec(AgentSpec::new(
            "shared",
            "primary-model",
            "primary body",
            serde_json::json!({"type": "object"}),
            serde_json::json!({"type": "object"}),
        ));
        secondary.insert_spec(AgentSpec::new(
            "shared",
            "secondary-model",
            "secondary body",
            serde_json::json!({"type": "object"}),
            serde_json::json!({"type": "object"}),
        ));
        secondary.insert_spec(AgentSpec::new(
            "secondary-only",
            "m",
            "x",
            serde_json::json!({"type": "object"}),
            serde_json::json!({"type": "object"}),
        ));

        let chained = ChainedAgentRegistry::new(vec![
            primary as Arc<dyn AgentRegistry>,
            secondary as Arc<dyn AgentRegistry>,
        ]);

        let resolved = chained.get("shared").await.expect("found");
        assert_eq!(resolved.spec().model, "primary-model");
        assert!(chained.get("secondary-only").await.is_some());
        assert!(chained.get("missing").await.is_none());

        let mut names = chained.names().await;
        names.sort();
        assert_eq!(
            names,
            vec!["secondary-only".to_string(), "shared".to_string()]
        );
    }
}
