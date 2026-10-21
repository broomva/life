//! Cuenca's working-directory side: turn any directory into a [`cuenca_tree::Tree`] and a
//! [`cuenca_tree::Changeset`], and materialise a tree into a directory.
//!
//! A location is any directory tree, not only a git repository, so nothing here calls
//! git. A [`snapshot`] is always a full-tree scan of the directory against its last
//! snapshot (spec section 2): no change notification is trusted to say what changed. A
//! [`StatCache`] makes the scan cheap, by hashing only files whose stat changed since
//! the cache recorded them. Paths the location's [`ForkIgnore`] names never enter a
//! snapshot.
//!
//! ```no_run
//! use cuenca_fs::{StatCache, checkout, snapshot, CheckoutSource};
//! use cuenca_tree::Tree;
//! # fn main() -> Result<(), cuenca_fs::FsError> {
//! let mut cache = StatCache::new();
//! let snap = snapshot("/work/site".as_ref(), &Tree::new(), &mut cache)?;
//! // Publish snap.changeset and snap.new_blobs, then fork it elsewhere:
//! let source = CheckoutSource::new("/work/site", &snap.tree, &cache);
//! let fork = checkout(&snap.tree, "/work/site-fork".as_ref(), &source)?;
//! assert_eq!(fork.cache.len(), cache.len());
//! # Ok(()) }
//! ```
//!
//! # What a snapshot holds
//!
//! Regular files (executable when the owner's execute bit is set, as git reads it) and
//! symbolic links, which are never followed. Directories are implicit, so an empty
//! directory is not recorded. A `.git` entry at any depth is skipped silently: it is a
//! repository's own state, and the format cannot hold the name. Anything else a tree
//! cannot hold fails the snapshot loudly with its path, so nothing is lost without a
//! word: a name that is not UTF-8 or that the path rules refuse, two names that collide
//! on a case-insensitive disk, an unreadable file, or a file that keeps changing while it
//! is read. Sockets, FIFOs and device files are not content; they are left out and
//! listed in [`Snapshot::skipped`].
//!
//! # Trust
//!
//! The stat cache is the checkout's own local state, trusted as git trusts its index:
//! a file whose stat matches its cached entry exactly is not read. An entry is cached
//! only once its modification time is [`RACY_MARGIN`] older than the scan that recorded
//! it, so a write landing within the clock's granularity after the scan cannot hide
//! behind an unchanged stat (git's "racy git" rule). A checkout gives every file it
//! writes a modification time that old, so the checkout's first snapshot reads nothing.
//!
//! A [`BlobSource`] is trusted to return the content it is asked for when it hands back
//! a file to clone: [`ObjectDir`] is the authority's blob store, which verified every
//! blob at ingest, and [`CheckoutSource`] offers a file only while its stat matches the
//! stat cache, and checkout re-checks that stat after cloning. Bytes handed back in
//! memory are hashed before they are written.

mod cache;
mod checkout;
mod error;
mod ignore;
mod scan;

pub use cache::{RACY_MARGIN, Stamp, StatCache};
pub use checkout::{BlobSource, Checkout, CheckoutSource, Fetched, ObjectDir, checkout};
pub use error::FsError;
pub use ignore::{DEFAULT_FORKIGNORE, FORKIGNORE, ForkIgnore};
pub use scan::{Content, MAX_FILE_BYTES, NewBlob, Skipped, Snapshot, snapshot, snapshot_with};
