use crate::error::PathError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Borrow;
use std::fmt;
use unicode_normalization::UnicodeNormalization;

/// A path inside a repository: UTF-8, `/`-separated, relative, with no empty, `.` or
/// `..` component and no component git reserves for itself.
///
/// Ordering is byte order of the UTF-8 string, which is the canonical order of a
/// changeset's ops. Git's own tree order is per directory and is applied separately
/// when tree objects are built.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath(String);

impl RepoPath {
    pub fn new(path: impl Into<String>) -> Result<Self, PathError> {
        let path = path.into();
        if path.is_empty() {
            return Err(PathError::Empty);
        }
        if path.contains('\0') {
            return Err(PathError::Nul(path));
        }
        if path.starts_with('/') || path.ends_with('/') {
            return Err(PathError::Slash(path));
        }
        for component in path.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                return Err(PathError::BadComponent(path));
            }
            if is_git_dir_alias(component) {
                return Err(PathError::GitDir(path));
            }
        }
        Ok(Self(path))
    }

    /// Parse raw bytes, as git plumbing prints them.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PathError> {
        let s = std::str::from_utf8(bytes).map_err(|_| PathError::NotUtf8)?;
        Self::new(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The directory part and the final component: `a/b/c` → (`a/b`, `c`), `c` → (``, `c`).
    pub fn split_last(&self) -> (&str, &str) {
        match self.0.rfind('/') {
            Some(i) => (&self.0[..i], &self.0[i + 1..]),
            None => ("", &self.0),
        }
    }

    /// Every directory that contains this path, outermost first: `a/b/c` → `a`, `a/b`.
    pub fn ancestors(&self) -> impl Iterator<Item = &str> {
        self.0.match_indices('/').map(|(i, _)| &self.0[..i])
    }
}

/// The key two names share when a case-insensitive, normalisation-insensitive
/// filesystem (APFS's default, and SMB clients on it) would treat them as one name.
pub(crate) fn fold_key(name: &str) -> String {
    name.nfd()
        .collect::<String>()
        .to_lowercase()
        .nfc()
        .collect()
}

/// Code points HFS+ and APFS ignore when comparing names, which git also skips when it
/// protects `.git` on macOS (`core.protectHFS`).
fn is_hfs_ignorable(c: char) -> bool {
    matches!(c, '\u{200C}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{206A}'..='\u{206F}' | '\u{FEFF}')
}

/// `.git` under every spelling a checkout on macOS or Windows would resolve to the
/// repository's own directory: any case, HFS-ignorable code points inserted, NTFS
/// trailing dots and spaces, and the NTFS short name `git~1`.
fn is_git_dir_alias(component: &str) -> bool {
    let visible: String = component
        .chars()
        .filter(|c| !is_hfs_ignorable(*c))
        .collect();
    let lower = visible.to_lowercase();
    let ntfs_trimmed = lower.trim_end_matches(['.', ' ']);
    ntfs_trimmed == ".git" || lower == "git~1"
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl Borrow<str> for RepoPath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl Serialize for RepoPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_paths() {
        for p in [
            "a",
            "a/b/c.txt",
            "sp ace/tab\tq'uo\"te",
            "nl\nx/🦀",
            ".gitignore",
            "a/.github/x",
            "gitx",
        ] {
            assert!(RepoPath::new(p).is_ok(), "{p:?}");
        }
    }

    #[test]
    fn rejects_malformed_paths() {
        for p in [
            "", "/a", "a/", "a//b", "./a", "a/./b", "a/../b", "..", "a\0b",
        ] {
            assert!(RepoPath::new(p).is_err(), "{p:?}");
        }
    }

    #[test]
    fn rejects_every_git_dir_alias() {
        for p in [
            ".git",
            ".git/config",
            "a/.GIT/x",
            "a/.Git",
            ".g\u{200C}it",
            ".git.",
            ".git ",
            "x/GIT~1/y",
        ] {
            assert_eq!(
                RepoPath::new(p),
                Err(PathError::GitDir(p.to_string())),
                "{p:?}"
            );
        }
    }

    #[test]
    fn non_utf8_bytes_are_rejected() {
        assert_eq!(RepoPath::from_bytes(b"a/\xff"), Err(PathError::NotUtf8));
    }

    #[test]
    fn split_and_ancestors() {
        let p = RepoPath::new("a/b/c").unwrap();
        assert_eq!(p.split_last(), ("a/b", "c"));
        assert_eq!(p.ancestors().collect::<Vec<_>>(), ["a", "a/b"]);
        let root = RepoPath::new("c").unwrap();
        assert_eq!(root.split_last(), ("", "c"));
        assert_eq!(root.ancestors().count(), 0);
    }

    #[test]
    fn fold_key_merges_case_and_normalisation() {
        assert_eq!(fold_key("README.md"), fold_key("readme.md"));
        assert_eq!(fold_key("caf\u{e9}"), fold_key("cafe\u{301}"));
        assert_eq!(fold_key("CAF\u{c9}"), fold_key("cafe\u{301}"));
        assert_ne!(fold_key("a"), fold_key("b"));
    }
}
