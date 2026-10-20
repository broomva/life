//! Git-isomorphic trees and changesets for Cuenca, the agent-native change plane.
//!
//! A [`Tree`] is a flat map from a validated [`RepoPath`] to an [`Entry`]. Each entry
//! is exactly what git's tree records, its mode and its blob's git id, so a tree is
//! identified by its git tree id. Directories are implicit, as in git. Lago stores
//! content under its SHA-256, which [`BlobHasher`] derives from verified bytes.
//!
//! A [`Changeset`] names the tree it applies to and the tree it produces, both as git
//! tree ids, so any party can check it without trusting the sender:
//!
//! ```
//! use cuenca_tree::{Changeset, Entry, Op, RepoPath, Tree};
//!
//! let base = Tree::new();
//! let path = RepoPath::new("src/main.rs").unwrap();
//! let change = Changeset::new(&base, vec![Op::put(path, Entry::file(b"fn main() {}\n"))]).unwrap();
//! let next = change.apply(&base).unwrap();
//! assert_eq!(next.git_tree_id(), change.target);
//! ```
//!
//! [`Tree::git_tree_id`] is computed in-process with git's object format, sort order and
//! mode strings. The [`git`] module bridges to a real repository through git plumbing,
//! and is how the round trip is tested against git itself.
//!
//! # Untrusted input
//!
//! Trees and changesets arrive from agents, so every constructor and every
//! deserializer validates: path rules (git's own `.git` rules for macOS and Windows,
//! length limits, no backslash), names that would collide on APFS under full Unicode
//! case folding, no `.gitmodules` anywhere and `.gitattributes` only as a file. Entries
//! name content by git id alone, so a changeset makes no claim about content beyond
//! that id; whoever accepts it checks the content it receives hashes to the id (the git
//! bridge does) and derives the SHA-256 Lago stores it under from those bytes.
//!
//! # Scope
//!
//! SHA-1 git with UTF-8 paths, and the spec's entry kinds: file, executable, symlink,
//! and implicit directories. Submodules, empty directories, SHA-256 object ids and
//! non-UTF-8 paths are refused.

mod changeset;
mod entry;
mod error;
pub mod git;
mod oid;
mod path;
mod tree;

pub use changeset::{Changeset, FORMAT, Op};
pub use entry::{Blob, BlobHasher, Entry, HashFailure, Mode};
pub use error::{ChangesetError, PathError, SymlinkProblem, TreeError};
pub use oid::{ContentHash, GitOid};
pub use path::{MAX_COMPONENT_BYTES, MAX_PATH_BYTES, RepoPath};
pub use tree::{
    MAX_GITATTRIBUTES_BYTES, MAX_GITATTRIBUTES_LINE, MAX_SYMLINK_TARGET, Tree, TreeObject,
    check_content, has_content_rule,
};
