use crate::error::PathError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Borrow;
use std::fmt;
use unicase::UniCase;
use unicode_normalization::UnicodeNormalization;

/// The longest path accepted, in bytes: macOS's `PATH_MAX` is 1024 including the NUL,
/// and a longer path cannot be checked out on a Mac.
pub const MAX_PATH_BYTES: usize = 1023;
/// The longest component accepted, in bytes: APFS and ext4 both stop at 255.
pub const MAX_COMPONENT_BYTES: usize = 255;

/// A path inside a repository: UTF-8, `/`-separated, relative, at most
/// [`MAX_PATH_BYTES`] long with components of at most [`MAX_COMPONENT_BYTES`], with no
/// empty, `.` or `..` component, no backslash, and no `.git` alias git refuses with
/// `core.protectHFS` and `core.protectNTFS` on (see [`RepoPath::new`]). Windows-only
/// names (`CON`, `a:b`, trailing dots) are not refused; Windows is not a target.
///
/// Ordering is byte order of the UTF-8 string, which is the canonical order of a
/// changeset's ops. Git's own tree order is per directory and is applied separately
/// when tree objects are built.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath(String);

impl RepoPath {
    /// Validate a path. The `.git` rules are git's own `verify_path` with
    /// `core.protectHFS` and `core.protectNTFS` both on, ported from git 2.50's
    /// `is_hfs_dotgit` and `is_ntfs_dotgit`: any case, HFS-ignorable code points
    /// inserted, NTFS trailing dots and spaces, NTFS alternate data streams (`.git:x`,
    /// `.git::$INDEX_ALLOCATION`) and the short name `git~1`. Backslashes are refused
    /// outright, since Windows and SMB both read them as separators.
    pub fn new(path: impl Into<String>) -> Result<Self, PathError> {
        let path = path.into();
        if path.is_empty() {
            return Err(PathError::Empty);
        }
        if path.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong(path.len()));
        }
        if path.contains('\0') {
            return Err(PathError::Nul(path));
        }
        if path.contains('\\') {
            return Err(PathError::Backslash(path));
        }
        if path.starts_with('/') || path.ends_with('/') {
            return Err(PathError::Slash(path));
        }
        for component in path.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                return Err(PathError::BadComponent(path));
            }
            if component.len() > MAX_COMPONENT_BYTES {
                return Err(PathError::TooLong(component.len()));
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

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// The directory part and the final component: `a/b/c` → (`a/b`, `c`), `c` → (``, `c`).
    pub fn split_last(&self) -> (&str, &str) {
        match self.0.rfind('/') {
            Some(i) => (&self.0[..i], &self.0[i + 1..]),
            None => ("", &self.0),
        }
    }

    /// The final component.
    pub fn file_name(&self) -> &str {
        self.split_last().1
    }
}

/// The key two names share when a case-insensitive, normalisation-insensitive
/// filesystem (APFS's default, and SMB clients on it) would treat them as one name:
/// Unicode canonical caseless matching, `NFD(fold(NFD(name)))`, with full case folding
/// (so `ß` meets `ss`, `ſ` meets `s` and `ﬁ` meets `fi`). Apply it to one component.
pub(crate) fn fold_component(name: &str) -> String {
    let decomposed: String = name.nfd().collect();
    UniCase::unicode(decomposed)
        .to_folded_case()
        .nfd()
        .collect()
}

/// Code points HFS+ and APFS ignore when comparing names; git skips them when it
/// protects `.git` on macOS (`next_hfs_char` in git's utf8.c).
fn is_hfs_ignorable(c: char) -> bool {
    matches!(c, '\u{200C}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{206A}'..='\u{206F}' | '\u{FEFF}')
}

/// git's `is_hfs_dot_generic`: `.` then `needle`, ASCII case-insensitive, ignoring
/// HFS-ignorable code points, and nothing after.
fn is_hfs_dot(component: &str, needle: &str) -> bool {
    let mut chars = component.chars().filter(|c| !is_hfs_ignorable(*c));
    if chars.next() != Some('.') {
        return false;
    }
    for n in needle.chars() {
        match chars.next() {
            Some(c) if c.is_ascii() && c.to_ascii_lowercase() == n => {}
            _ => return false,
        }
    }
    chars.next().is_none()
}

/// The tail git's NTFS checks accept after a reserved name: only spaces and dots, up to
/// the end or an alternate-data-stream `:`.
fn only_spaces_and_periods(rest: &[u8]) -> bool {
    for &c in rest {
        if c == b':' {
            return true;
        }
        if c != b' ' && c != b'.' {
            return false;
        }
    }
    true
}

/// git's `is_ntfs_dotgit`: `.git` or `git~1`, any case, then only spaces and dots.
fn is_ntfs_dotgit(name: &[u8]) -> bool {
    let rest = if name.len() >= 4 && name[0] == b'.' && name[1..4].eq_ignore_ascii_case(b"git") {
        &name[4..]
    } else if name.len() >= 5 && name[..3].eq_ignore_ascii_case(b"git") && &name[3..5] == b"~1" {
        &name[5..]
    } else {
        return false;
    };
    only_spaces_and_periods(rest)
}

/// git's `is_ntfs_dot_generic`: `.<needle>`, its 6-character short name `~1`..`~4`, or
/// the fall-back short name `<prefix>~N`, each followed only by spaces and dots.
fn is_ntfs_dot_generic(name: &[u8], needle: &[u8], short_prefix: &[u8]) -> bool {
    if name.len() > needle.len()
        && name[0] == b'.'
        && name[1..=needle.len()].eq_ignore_ascii_case(needle)
    {
        return only_spaces_and_periods(&name[needle.len() + 1..]);
    }
    if name.len() >= 8
        && name[..6].eq_ignore_ascii_case(&needle[..6])
        && name[6] == b'~'
        && (b'1'..=b'4').contains(&name[7])
    {
        return only_spaces_and_periods(&name[8..]);
    }
    let mut i = 0;
    let mut saw_tilde = false;
    while i < 8 {
        let Some(&c) = name.get(i) else { return false };
        if saw_tilde {
            if !c.is_ascii_digit() {
                return false;
            }
        } else if c == b'~' {
            i += 1;
            match name.get(i) {
                Some(d) if (b'1'..=b'9').contains(d) => saw_tilde = true,
                _ => return false,
            }
        } else if i >= 6 || c & 0x80 != 0 || c.to_ascii_lowercase() != short_prefix[i] {
            return false;
        }
        i += 1;
    }
    only_spaces_and_periods(name.get(i..).unwrap_or(&[]))
}

/// `.git` under every spelling a checkout on macOS or Windows resolves to the
/// repository's own directory.
fn is_git_dir_alias(component: &str) -> bool {
    is_hfs_dot(component, "git") || is_ntfs_dotgit(component.as_bytes())
}

/// Whether `component`, with HFS-ignorable code points dropped, is `needle` once case
/// and normalisation are folded as APFS folds them: `.gitmoduleſ` is `.gitmodules` on a
/// Mac, though git's ASCII-only checks do not see it.
fn folds_to(component: &str, needle: &str) -> bool {
    let visible: String = component
        .chars()
        .filter(|c| !is_hfs_ignorable(*c))
        .collect();
    fold_component(&visible) == fold_component(needle)
}

/// `.gitmodules` under every spelling git's `verify_path` and `fsck` recognise
/// (`is_hfs_dotgitmodules`, `is_ntfs_dotgitmodules`), checked on one component.
pub(crate) fn is_gitmodules_alias(component: &str) -> bool {
    is_hfs_dot(component, "gitmodules")
        || is_ntfs_dot_generic(component.as_bytes(), b"gitmodules", b"gi7eba")
        || folds_to(component, ".gitmodules")
}

/// `.gitattributes` under every spelling git's `fsck` recognises
/// (`is_hfs_dotgitattributes`, `is_ntfs_dotgitattributes`), checked on one component.
pub(crate) fn is_gitattributes_alias(component: &str) -> bool {
    is_hfs_dot(component, "gitattributes")
        || is_ntfs_dot_generic(component.as_bytes(), b"gitattributes", b"gi7d29")
        || folds_to(component, ".gitattributes")
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
            ".gitx",
            "git~2",
            ".gitmodules",
        ] {
            assert!(RepoPath::new(p).is_ok(), "{p:?}");
        }
    }

    #[test]
    fn rejects_malformed_paths() {
        for p in [
            "", "/a", "a/", "a//b", "./a", "a/./b", "a/../b", "..", "a\0b", "a\\b",
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
            ".git . .",
            "x/GIT~1/y",
            "git~1.",
            "GIT~1 ",
            "git~1:stream/config",
            ".git:x",
            ".git::$INDEX_ALLOCATION/config",
            ".git :x",
        ] {
            assert_eq!(
                RepoPath::new(p),
                Err(PathError::GitDir(p.to_string())),
                "{p:?}"
            );
        }
    }

    #[test]
    fn enforces_length_limits() {
        assert!(RepoPath::new("a".repeat(MAX_COMPONENT_BYTES)).is_ok());
        assert_eq!(RepoPath::new("a".repeat(256)), Err(PathError::TooLong(256)));
        let deep = vec!["a"; 512].join("/");
        assert_eq!(deep.len(), 1023);
        assert!(RepoPath::new(deep.clone()).is_ok());
        let over = format!("{deep}b");
        assert_eq!(RepoPath::new(over.clone()), Err(PathError::TooLong(1024)));
    }

    #[test]
    fn gitmodules_aliases() {
        for name in [
            ".gitmodules",
            ".GITMODULES",
            ".gitmodules.",
            ".gitmodules :x",
            "gitmod~1",
            "GITMOD~4",
            "gi7eba~9",
            ".g\u{200D}itmodules",
            ".gitmodule\u{17F}",
        ] {
            assert!(is_gitmodules_alias(name), "{name:?}");
        }
        assert!(is_gitattributes_alias(".gitattribute\u{17F}"));
        for name in [".gitmodulesx", "gitmod~5", "gitmodules", "gi7eba~0"] {
            assert!(!is_gitmodules_alias(name), "{name:?}");
        }
    }

    #[test]
    fn non_utf8_bytes_are_rejected() {
        assert_eq!(RepoPath::from_bytes(b"a/\xff"), Err(PathError::NotUtf8));
    }

    #[test]
    fn split_and_components() {
        let p = RepoPath::new("a/b/c").unwrap();
        assert_eq!(p.split_last(), ("a/b", "c"));
        assert_eq!(p.components().collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(RepoPath::new("c").unwrap().split_last(), ("", "c"));
    }

    #[test]
    fn fold_merges_case_normalisation_and_full_folds() {
        for (a, b) in [
            ("README.md", "readme.md"),
            ("caf\u{e9}", "cafe\u{301}"),
            ("CAF\u{c9}", "cafe\u{301}"),
            ("straße", "strasse"),
            ("aς", "aσ"),
            ("ΑΣ", "ασ"),
            ("ﬁ", "fi"),
            ("ϐ", "β"),
            ("ſ", "s"),
            ("µ", "μ"),
            ("\u{345}", "ι"),
            ("ϑ", "θ"),
            ("ϰ", "κ"),
            ("ϱ", "ρ"),
            ("ϵ", "ε"),
            ("ﬃ", "ffi"),
            ("ﬄ", "ffl"),
        ] {
            assert_eq!(fold_component(a), fold_component(b), "{a:?} vs {b:?}");
        }
        assert_ne!(fold_component("a"), fold_component("b"));
        // unicase 2.9.0 folded three-character ligatures backwards; 2.10.0 is pinned.
        assert_ne!(fold_component("ﬃ"), fold_component("iff"));
    }
}
