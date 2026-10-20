//! Git-isomorphic trees and changesets for Cuenca, the agent-native change plane.
//!
//! A [`Tree`] is a flat map from a validated [`RepoPath`] to an [`Entry`]. Each entry
//! carries what git needs to rebuild the identical tree object (its kind and git blob
//! id) plus the SHA-256 of its content, the key Lago's blob store uses. Directories are
//! implicit, as in git.
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
//! case folding, symlinked `.gitmodules`, and one SHA-256 and size per blob id. A
//! blob's SHA-256 and size are claims until its content is checked with
//! [`Blob::matches`]; the git bridge checks every blob it reads or writes.
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
pub use entry::{Blob, BlobHasher, Entry, Mode};
pub use error::{ChangesetError, PathError, TreeError};
pub use oid::{ContentHash, GitOid};
pub use path::{MAX_COMPONENT_BYTES, MAX_PATH_BYTES, RepoPath};
pub use tree::{Tree, TreeObject};
