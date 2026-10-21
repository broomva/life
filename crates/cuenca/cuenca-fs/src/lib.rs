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
//! repository's own state, and the format cannot hold the name. The `.forkignore` rules
//! apply before anything else, so they can leave out any name. Anything else a tree
//! cannot hold fails the snapshot loudly with its path, so nothing is lost without a
//! word: a name that is not UTF-8 or that the path rules refuse, two names that collide
//! on a case-insensitive disk, an unreadable file, or a file that keeps changing while it
//! is read. Sockets, FIFOs and devices are not content; they are left out and listed in
//! [`Snapshot::skipped`]. A path of the base that the rules leave out keeps its base
//! entry: the snapshot did not look at it, so it does not claim it changed.
//!
//! The walk reaches everything relative to open directory descriptors, opens no file
//! through a link and a directory only if it is the inode the listing saw, so a directory
//! swapped for a symlink mid-scan cannot lead it outside the root. A checkout writes
//! relative to one descriptor for its destination, which must be the caller's own and
//! writable by no one else.
//!
//! # Trust
//!
//! The stat cache is the checkout's own local state, trusted as git trusts its index:
//! a file whose stat matches its cached entry exactly is not read. An entry is cached
//! only once its modification time is [`RACY_MARGIN`] older than the scan that recorded
//! it, so a write landing within the clock's granularity after the scan cannot hide
//! behind an unchanged stat (git's "racy git" rule). The scan's clock is the host's, so a
//! location on a network filesystem whose server clock runs more than the margin behind
//! weakens this rule.
//!
//! A checkout's new cache vouches only for what was verified: content handed over in
//! memory, which is hashed before it is written, and clones of files a source vouched for
//! by stat ([`CheckoutSource`], whose stat the checkout re-checks after cloning). A file
//! cloned from [`ObjectDir`], the authority's blob store, is taken on the store's word
//! (it verified every blob at ingest) and is left out of the new cache, so the first
//! snapshot of the checkout reads it.

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
