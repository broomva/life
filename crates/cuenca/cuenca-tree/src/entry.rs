use crate::error::TreeError;
use crate::oid::{ContentHash, GitOid};
use crate::oid::{checked_sha1, finish_checked};
use serde::{Deserialize, Serialize};
use sha1_checked::Sha1;
use sha2::Digest as _;
use sha2::Sha256;

/// What hashing a blob's content yields: its git id, the SHA-256 Lago stores it under,
/// and its size.
///
/// A `Blob` is only ever computed from content ([`Blob::of`], [`BlobHasher`]); it is not
/// part of any wire format, so it cannot arrive as a claim. Trees and changesets name
/// content by git id alone, and whoever stores content derives the SHA-256 from the
/// bytes it verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blob {
    oid: GitOid,
    sha256: ContentHash,
    size: u64,
}

impl Blob {
    /// Hash `content`, or `None` if SHA-1 collision detection fires on it.
    pub fn of(content: &[u8]) -> Option<Self> {
        let mut h = BlobHasher::new(content.len() as u64);
        h.update(content);
        h.finish().ok()
    }

    pub fn oid(&self) -> GitOid {
        self.oid
    }

    pub fn sha256(&self) -> ContentHash {
        self.sha256
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

/// Why hashing a stream did not yield a [`Blob`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HashFailure {
    /// The stream held fewer or more bytes than announced.
    #[error("expected {expected} bytes, saw {seen}")]
    WrongLength { expected: u64, seen: u64 },
    /// SHA-1 collision detection fired, as git's SHA-1DC would.
    #[error("SHA-1 collision detected")]
    Collision,
}

/// Hashes a blob's content as it streams, so a large blob is never held in memory.
pub struct BlobHasher {
    git: Sha1,
    sha256: Sha256,
    expected: u64,
    seen: u64,
}

impl BlobHasher {
    /// Start hashing a blob of `size` bytes; git's id covers the size up front.
    pub fn new(size: u64) -> Self {
        Self::with_header(format!("blob {size}\0").as_bytes(), size)
    }

    /// The hasher with an arbitrary object header; `new` uses git's blob header. Kept
    /// separate so a test can feed a known SHA-1 collision through the same path.
    fn with_header(header: &[u8], size: u64) -> Self {
        let mut git = checked_sha1();
        sha1_checked::Digest::update(&mut git, header);
        Self {
            git,
            sha256: Sha256::new(),
            expected: size,
            seen: 0,
        }
    }

    pub fn update(&mut self, chunk: &[u8]) {
        sha1_checked::Digest::update(&mut self.git, chunk);
        self.sha256.update(chunk);
        self.seen += chunk.len() as u64;
    }

    /// The bytes seen so far.
    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// The blob, unless the content was not the size given to [`BlobHasher::new`] or
    /// SHA-1 collision detection fired.
    pub fn finish(self) -> Result<Blob, HashFailure> {
        if self.seen != self.expected {
            return Err(HashFailure::WrongLength {
                expected: self.expected,
                seen: self.seen,
            });
        }
        let oid = finish_checked(self.git).ok_or(HashFailure::Collision)?;
        Ok(Blob {
            oid: GitOid::from_bytes(oid),
            sha256: ContentHash::from_bytes(self.sha256.finalize().into()),
            size: self.seen,
        })
    }
}

/// One path's entry: its git mode and the git id of its content. That is exactly what
/// git's tree records, so two entries are equal exactly when git sees them as equal.
///
/// The kinds are the spec's pinned set (section 3): regular file, executable and
/// symlink; directories are implicit. Submodules (mode `160000`) are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Entry {
    /// A regular file, git mode `100644`.
    File(GitOid),
    /// An executable file, git mode `100755`.
    Executable(GitOid),
    /// A symbolic link, git mode `120000`. The blob is the link's target.
    Symlink(GitOid),
}

impl Entry {
    /// The entry for `content`, by plain SHA-1. For content the caller already trusts;
    /// untrusted bytes go through [`Blob::of`] or [`BlobHasher`], which detect collisions.
    pub fn file(content: &[u8]) -> Self {
        Self::File(GitOid::for_blob(content))
    }

    pub fn executable(content: &[u8]) -> Self {
        Self::Executable(GitOid::for_blob(content))
    }

    pub fn symlink(target: &[u8]) -> Self {
        Self::Symlink(GitOid::for_blob(target))
    }

    pub fn mode(&self) -> Mode {
        match self {
            Self::File(_) => Mode::File,
            Self::Executable(_) => Mode::Executable,
            Self::Symlink(_) => Mode::Symlink,
        }
    }

    /// The git id of the entry's content.
    pub fn oid(&self) -> GitOid {
        match self {
            Self::File(oid) | Self::Executable(oid) | Self::Symlink(oid) => *oid,
        }
    }

    /// Build an entry from a git mode and the blob id it names.
    pub fn from_mode(mode: Mode, oid: GitOid) -> Result<Self, TreeError> {
        match mode {
            Mode::File => Ok(Self::File(oid)),
            Mode::Executable => Ok(Self::Executable(oid)),
            Mode::Symlink => Ok(Self::Symlink(oid)),
            Mode::Tree => Err(TreeError::UnsupportedMode(mode.as_str().to_string())),
        }
    }
}

/// The git modes a tree entry can have here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Mode {
    File,
    Executable,
    Symlink,
    Tree,
}

impl Mode {
    /// The mode as git writes it in a tree object (trees have no leading zero).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "100644",
            Self::Executable => "100755",
            Self::Symlink => "120000",
            Self::Tree => "40000",
        }
    }

    /// The object type git plumbing prints next to the mode.
    pub fn object_type(&self) -> &'static str {
        match self {
            Self::File | Self::Executable | Self::Symlink => "blob",
            Self::Tree => "tree",
        }
    }

    /// Parse a mode as `git ls-tree` prints it. Submodules (`160000`) are refused, as
    /// the spec's format has no submodules. (`ls-tree` already prints legacy modes such
    /// as `100664` as `100644`, and git computes the tree id from what it stores, so a
    /// stored legacy mode surfaces as a tree id mismatch.)
    pub fn parse(s: &str) -> Result<Self, TreeError> {
        match s {
            "100644" => Ok(Self::File),
            "100755" => Ok(Self::Executable),
            "120000" => Ok(Self::Symlink),
            "040000" | "40000" => Ok(Self::Tree),
            "160000" => Err(TreeError::UnsupportedMode("160000 (submodule)".into())),
            other => Err(TreeError::UnsupportedMode(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_hash_equals_one_shot() {
        let content = b"hello\nworld\n";
        let mut h = BlobHasher::new(content.len() as u64);
        h.update(&content[..3]);
        h.update(&content[3..]);
        assert_eq!(h.finish().ok(), Blob::of(content));
        assert_eq!(Blob::of(content).unwrap().oid(), GitOid::for_blob(content));
        let mut short = BlobHasher::new(99);
        short.update(content);
        assert_eq!(
            short.finish(),
            Err(HashFailure::WrongLength {
                expected: 99,
                seen: 12
            })
        );
    }

    #[test]
    fn the_blob_hasher_refuses_a_known_collision() {
        let colliding = include_bytes!("../tests/data/sha-mbles-1.bin");
        let mut h = BlobHasher::with_header(b"", colliding.len() as u64);
        h.update(colliding);
        assert_eq!(h.finish(), Err(HashFailure::Collision));
    }

    #[test]
    fn entries_serialize_as_git_identity_only() {
        let json = serde_json::to_string(&Entry::file(b"x")).unwrap();
        assert_eq!(json, format!("{{\"file\":\"{}\"}}", GitOid::for_blob(b"x")));
    }

    #[test]
    fn submodules_are_refused() {
        assert!(matches!(
            Mode::parse("160000"),
            Err(TreeError::UnsupportedMode(_))
        ));
    }
}
