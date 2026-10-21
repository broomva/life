//! `.forkignore`: the host-local paths a snapshot never publishes.

use crate::error::{FsError, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rustix::fs::{CWD, Mode, OFlags, openat};
use std::fs::File;
use std::io::Read as _;
use std::os::fd::OwnedFd;
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
    /// file that exists is the whole rule set: the location may drop a default. It must be
    /// a regular file; a symlink is refused rather than followed.
    pub fn for_root(root: &Path) -> Result<Self> {
        let dir = open_dir(root)?;
        Ok(Self::load(&dir, root)?.0)
    }

    /// [`ForkIgnore::for_root`] through an open root directory, with the bytes the rules
    /// were parsed from (`None` when the defaults apply).
    pub(crate) fn load(dir: &OwnedFd, root: &Path) -> Result<(Self, Option<Vec<u8>>)> {
        let path = root.join(FORKIGNORE);
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let fd = match openat(dir, FORKIGNORE, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok((Self::defaults(), None)),
            Err(rustix::io::Errno::LOOP) => return Err(not_a_file()),
            Err(e) => return Err(FsError::io(path)(e.into())),
        };
        let mut file = File::from(fd);
        if !file.metadata().map_err(FsError::io(&path))?.is_file() {
            return Err(not_a_file());
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(FsError::io(&path))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| FsError::Ignore("the file is not UTF-8".into()))?;
        Ok((Self::parse(text)?, Some(bytes)))
    }

    /// Whether `path` (relative to the root) is ignored.
    pub fn is_ignored(&self, path: impl AsRef<Path>, is_dir: bool) -> bool {
        self.matcher.matched(path, is_dir).is_ignore()
    }

    /// Whether the rules name the file at `path`: the file is ignored, or one of its
    /// directories is (a later `!` rule cannot bring back a path under an ignored
    /// directory, as in git). A snapshot leaves such a path out unless its base already
    /// holds it.
    pub fn excludes(&self, path: &str) -> bool {
        let mut end = 0;
        while let Some(i) = path[end..].find('/') {
            end += i;
            if self.is_ignored(&path[..end], true) {
                return true;
            }
            end += 1;
        }
        self.is_ignored(path, false)
    }
}

fn not_a_file() -> FsError {
    FsError::Ignore(format!("{FORKIGNORE} must be a regular file"))
}

/// Open a directory for an fd-relative walk or checkout. `root` itself may be reached
/// through a symlink; what is under it is never followed.
pub(crate) fn open_dir(root: &Path) -> Result<OwnedFd> {
    openat(
        CWD,
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| FsError::io(root)(e.into()))
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

    #[test]
    fn excludes_matches_the_walk_under_an_ignored_directory() {
        let rules = ForkIgnore::parse("node_modules\n!node_modules/keep\n").unwrap();
        assert!(
            rules.excludes("node_modules/keep"),
            "git cannot re-include under an ignored dir"
        );
        assert!(rules.excludes("a/node_modules/x/y.js"));
        assert!(!rules.excludes("src/node_modules.rs"));
    }

    #[test]
    fn a_forkignore_that_is_a_symlink_or_a_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("rules"), b".env\n").unwrap();
        std::os::unix::fs::symlink("rules", dir.path().join(FORKIGNORE)).unwrap();
        assert!(matches!(
            ForkIgnore::for_root(dir.path()),
            Err(FsError::Ignore(_))
        ));
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(FORKIGNORE)).unwrap();
        assert!(matches!(
            ForkIgnore::for_root(dir.path()),
            Err(FsError::Ignore(_))
        ));
    }
}
