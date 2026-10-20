use crate::error::TreeError;
use crate::oid::{ContentHash, GitOid};
use serde::{Deserialize, Serialize};

/// A blob's identity: its git id, the SHA-256 Lago stores it under, and its size.
///
/// Both hashes are kept so the authority can bind content to a changeset without
/// relying on SHA-1 alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    pub oid: GitOid,
    pub sha256: ContentHash,
    pub size: u64,
}

impl Blob {
    pub fn of(content: &[u8]) -> Self {
        Self {
            oid: GitOid::for_blob(content),
            sha256: ContentHash::of(content),
            size: content.len() as u64,
        }
    }

    /// Whether `content` is this blob, by both hashes and the size.
    pub fn matches(&self, content: &[u8]) -> bool {
        *self == Self::of(content)
    }
}

/// One path's entry. The variant is the git mode, so a kind change is a change of entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entry {
    /// A regular file, git mode `100644`.
    File(Blob),
    /// An executable file, git mode `100755`.
    Executable(Blob),
    /// A symbolic link, git mode `120000`. The blob is the link's target.
    Symlink(Blob),
    /// A submodule commit, git mode `160000`. It has no content in this repository.
    Gitlink(GitOid),
}

impl Entry {
    pub fn file(content: &[u8]) -> Self {
        Self::File(Blob::of(content))
    }

    pub fn executable(content: &[u8]) -> Self {
        Self::Executable(Blob::of(content))
    }

    pub fn symlink(target: &[u8]) -> Self {
        Self::Symlink(Blob::of(target))
    }

    pub fn mode(&self) -> Mode {
        match self {
            Self::File(_) => Mode::File,
            Self::Executable(_) => Mode::Executable,
            Self::Symlink(_) => Mode::Symlink,
            Self::Gitlink(_) => Mode::Gitlink,
        }
    }

    /// The id git's tree records for this entry.
    pub fn oid(&self) -> GitOid {
        match self {
            Self::File(b) | Self::Executable(b) | Self::Symlink(b) => b.oid,
            Self::Gitlink(commit) => *commit,
        }
    }

    pub fn blob(&self) -> Option<&Blob> {
        match self {
            Self::File(b) | Self::Executable(b) | Self::Symlink(b) => Some(b),
            Self::Gitlink(_) => None,
        }
    }

    /// Build an entry from a git mode and the blob it names.
    pub fn from_mode(mode: Mode, blob: Blob) -> Result<Self, TreeError> {
        match mode {
            Mode::File => Ok(Self::File(blob)),
            Mode::Executable => Ok(Self::Executable(blob)),
            Mode::Symlink => Ok(Self::Symlink(blob)),
            Mode::Gitlink | Mode::Tree => {
                Err(TreeError::UnsupportedMode(mode.as_str().to_string()))
            }
        }
    }
}

/// The git modes a tree entry can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    File,
    Executable,
    Symlink,
    Gitlink,
    Tree,
}

impl Mode {
    /// The mode as git writes it in a tree object (trees have no leading zero).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "100644",
            Self::Executable => "100755",
            Self::Symlink => "120000",
            Self::Gitlink => "160000",
            Self::Tree => "40000",
        }
    }

    /// The object type git plumbing prints next to the mode.
    pub fn object_type(&self) -> &'static str {
        match self {
            Self::File | Self::Executable | Self::Symlink => "blob",
            Self::Gitlink => "commit",
            Self::Tree => "tree",
        }
    }

    /// Parse a mode as `git ls-tree` prints it. Legacy modes such as `100664` are refused,
    /// since rebuilding them would not round-trip through a checkout.
    pub fn parse(s: &str) -> Result<Self, TreeError> {
        match s {
            "100644" => Ok(Self::File),
            "100755" => Ok(Self::Executable),
            "120000" => Ok(Self::Symlink),
            "160000" => Ok(Self::Gitlink),
            "040000" | "40000" => Ok(Self::Tree),
            other => Err(TreeError::UnsupportedMode(other.to_string())),
        }
    }
}
