//! `.forkignore`: the host-local paths a snapshot never publishes.

use crate::error::{FsError, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::io;
use std::path::Path;

/// The file name, at a location's root.
pub const FORKIGNORE: &str = ".forkignore";

/// What a location without a `.forkignore` ignores, and what registering a location
/// writes (spec section 2): Finder and iOS metadata, installed dependencies, build
/// output and local secrets.
pub const DEFAULT_FORKIGNORE: &str = "\
# Paths a Cuenca snapshot never publishes (gitignore syntax). The location owns this file.
.DS_Store
._*
.Spotlight-V100
.Trashes
.fseventsd
node_modules
target
.venv
.env
";

/// A parsed `.forkignore`, in gitignore syntax. Only the file at the root counts; a
/// directory it names is not descended into, so, as in git, a later `!` rule cannot
/// bring back a path under it.
pub struct ForkIgnore {
    matcher: Gitignore,
}

impl ForkIgnore {
    /// Parse rules in gitignore syntax, refusing a pattern that does not compile.
    pub fn parse(text: &str) -> Result<Self> {
        let mut builder = GitignoreBuilder::new("");
        for line in text.lines() {
            builder
                .add_line(None, line)
                .map_err(|e| FsError::Ignore(e.to_string()))?;
        }
        let matcher = builder
            .build()
            .map_err(|e| FsError::Ignore(e.to_string()))?;
        Ok(Self { matcher })
    }

    /// [`DEFAULT_FORKIGNORE`].
    pub fn defaults() -> Self {
        Self::parse(DEFAULT_FORKIGNORE).expect("the default rules compile")
    }

    /// The rules of `root/.forkignore`, or the defaults when there is no such file. A
    /// file that exists is the whole rule set: the location may drop a default.
    pub fn for_root(root: &Path) -> Result<Self> {
        let path = root.join(FORKIGNORE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::defaults()),
            Err(e) => Err(FsError::Io { path, source: e }),
        }
    }

    /// Whether `path` (relative to the root, `/`-separated) is ignored.
    pub fn is_ignored(&self, path: &str, is_dir: bool) -> bool {
        self.matcher.matched(path, is_dir).is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_cover_the_spec_list_at_any_depth() {
        let rules = ForkIgnore::defaults();
        for (path, dir) in [
            (".DS_Store", false),
            ("a/b/.DS_Store", false),
            ("._x.txt", false),
            ("docs/._notes.md", false),
            (".Spotlight-V100", true),
            (".Trashes", true),
            (".fseventsd", true),
            ("node_modules", true),
            ("web/node_modules", true),
            ("target", true),
            ("crates/x/target", true),
            (".venv", true),
            (".env", false),
            ("app/.env", false),
        ] {
            assert!(rules.is_ignored(path, dir), "{path} should be ignored");
        }
        for path in ["src/main.rs", ".envrc", "env", "targets.md", "README.md"] {
            assert!(!rules.is_ignored(path, false), "{path} should be kept");
        }
    }

    #[test]
    fn a_bad_pattern_is_refused() {
        assert!(matches!(
            ForkIgnore::parse("[z-a]"),
            Err(FsError::Ignore(_))
        ));
    }

    #[test]
    fn negation_and_anchoring_follow_gitignore() {
        let rules = ForkIgnore::parse("*.log\n!keep.log\n/build\n").unwrap();
        assert!(rules.is_ignored("x.log", false));
        assert!(!rules.is_ignored("keep.log", false));
        assert!(rules.is_ignored("build", true));
        assert!(!rules.is_ignored("src/build", true));
    }
}
