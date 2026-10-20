use crate::entry::Entry;
use crate::error::ChangesetError;
use crate::oid::GitOid;
use crate::path::RepoPath;
use crate::tree::Tree;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// The changeset format this build writes and reads.
pub const FORMAT: u32 = 1;

/// One change to one path. A rename is a delete of the old path and a put of the new
/// one, as in git, which has no rename object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Op {
    Put { path: RepoPath, entry: Entry },
    Delete { path: RepoPath },
}

impl Op {
    pub fn put(path: RepoPath, entry: Entry) -> Self {
        Self::Put { path, entry }
    }

    pub fn delete(path: RepoPath) -> Self {
        Self::Delete { path }
    }

    pub fn path(&self) -> &RepoPath {
        match self {
            Self::Put { path, .. } | Self::Delete { path } => path,
        }
    }
}

/// A change from one tree to another. It names both trees by git tree id, so whoever
/// applies it can check that it starts from the tree they hold and ends where it says.
///
/// Ops are canonical: sorted by path, at most one per path, and each one changes what
/// git sees (mode or blob id). Two changesets between the same trees are therefore
/// equal.
///
/// A put names content by git id only. Whoever accepts a changeset must hold or
/// receive the content of every put's blob, check it hashes to that id, and apply
/// [`crate::check_content`] where [`crate::has_content_rule`] says so (once per blob id
/// and kind of path, not once per path); the SHA-256 Lago stores it under is derived
/// from those verified bytes, never taken from the sender. The git bridge's
/// `write_tree` does all of this and then reads the result back.
///
/// Canonical order is not a safe order for writing to a filesystem: with a base symlink
/// `d` and a put of `D/x`, byte order puts `D/x` first, and on a case-insensitive disk
/// writing it would follow the link. Whoever materialises a changeset deletes first and
/// never writes through a symlink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changeset {
    /// The format version, [`FORMAT`].
    pub format: u32,
    pub base: GitOid,
    pub target: GitOid,
    pub ops: Vec<Op>,
}

impl Changeset {
    /// Build a changeset from ops in any order, refusing two ops on one path.
    pub fn new(base: &Tree, mut ops: Vec<Op>) -> Result<Self, ChangesetError> {
        ops.sort_by(|a, b| a.path().cmp(b.path()));
        let mut change = Self {
            format: FORMAT,
            base: base.git_tree_id(),
            target: GitOid::empty_tree(),
            ops,
        };
        change.target = change.apply_ops(base)?.git_tree_id();
        Ok(change)
    }

    /// The canonical changeset that turns `base` into `target`.
    pub fn between(base: &Tree, target: &Tree) -> Self {
        let mut ops = Vec::new();
        let mut old = base.map().iter().peekable();
        let mut new = target.map().iter().peekable();
        loop {
            let order = match (old.peek(), new.peek()) {
                (None, None) => break,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some((a, _)), Some((b, _))) => a.cmp(b),
            };
            match order {
                Ordering::Less => {
                    let (path, _) = old.next().expect("peeked");
                    ops.push(Op::delete(path.clone()));
                }
                Ordering::Greater => {
                    let (path, entry) = new.next().expect("peeked");
                    ops.push(Op::put(path.clone(), *entry));
                }
                Ordering::Equal => {
                    let (_, before) = old.next().expect("peeked");
                    let (path, after) = new.next().expect("peeked");
                    if before != after {
                        ops.push(Op::put(path.clone(), *after));
                    }
                }
            }
        }
        Self {
            format: FORMAT,
            base: base.git_tree_id(),
            target: target.git_tree_id(),
            ops,
        }
    }

    /// Apply to `base`, checking that it is the tree this changeset starts from, that the
    /// ops are canonical, and that the result is the tree it names as its target.
    pub fn apply(&self, base: &Tree) -> Result<Tree, ChangesetError> {
        if self.format != FORMAT {
            return Err(ChangesetError::Format(self.format));
        }
        let actual = base.git_tree_id();
        if actual != self.base {
            return Err(ChangesetError::BaseMismatch {
                expected: self.base,
                actual,
            });
        }
        let result = self.apply_ops(base)?;
        let actual = result.git_tree_id();
        if actual != self.target {
            return Err(ChangesetError::TargetMismatch {
                expected: self.target,
                actual,
            });
        }
        Ok(result)
    }

    fn apply_ops(&self, base: &Tree) -> Result<Tree, ChangesetError> {
        let mut entries = base.map().clone();

        let mut previous: Option<&RepoPath> = None;
        for op in &self.ops {
            if previous.is_some_and(|p| p >= op.path()) {
                return Err(ChangesetError::NotCanonical(op.path().to_string()));
            }
            previous = Some(op.path());
            match op {
                Op::Put { path, entry } => {
                    if entries.insert(path.clone(), *entry) == Some(*entry) {
                        return Err(ChangesetError::NoOpPut(path.to_string()));
                    }
                }
                Op::Delete { path } => {
                    if entries.remove(path).is_none() {
                        return Err(ChangesetError::DeleteMissing(path.to_string()));
                    }
                }
            }
        }
        Ok(Tree::from_map(entries)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TreeError;

    fn p(s: &str) -> RepoPath {
        RepoPath::new(s).unwrap()
    }

    fn base() -> Tree {
        Tree::from_entries([
            (p("a.txt"), Entry::file(b"a")),
            (p("bin/run"), Entry::executable(b"#!/bin/sh\n")),
            (p("link"), Entry::symlink(b"a.txt")),
        ])
        .unwrap()
    }

    #[test]
    fn between_then_apply_reaches_the_target() {
        let base = base();
        let target = Tree::from_entries([
            (p("a.txt"), Entry::executable(b"a")),
            (p("b/c.txt"), Entry::file(b"c")),
            (p("link"), Entry::symlink(b"a.txt")),
        ])
        .unwrap();
        let change = Changeset::between(&base, &target);
        assert_eq!(
            change.ops.len(),
            3,
            "kind change, add, delete; the unchanged link is absent"
        );
        assert_eq!(change.apply(&base).unwrap(), target);
        assert_eq!(Changeset::between(&base, &base).ops, []);
    }

    #[test]
    fn new_sorts_ops_and_equals_between() {
        let base = base();
        let ops = vec![
            Op::delete(p("link")),
            Op::put(p("0first"), Entry::file(b"0")),
        ];
        let change = Changeset::new(&base, ops).unwrap();
        let target = change.apply(&base).unwrap();
        assert_eq!(change, Changeset::between(&base, &target));
    }

    #[test]
    fn refuses_the_wrong_base() {
        let change = Changeset::new(&base(), vec![Op::delete(p("link"))]).unwrap();
        assert!(matches!(
            change.apply(&Tree::new()),
            Err(ChangesetError::BaseMismatch { .. })
        ));
    }

    #[test]
    fn refuses_a_forged_target() {
        let mut change = Changeset::new(&base(), vec![Op::delete(p("link"))]).unwrap();
        change.target = GitOid::empty_tree();
        assert!(matches!(
            change.apply(&base()),
            Err(ChangesetError::TargetMismatch { .. })
        ));
    }

    #[test]
    fn refuses_non_canonical_ops() {
        let base = base();
        let mut change =
            Changeset::new(&base, vec![Op::delete(p("link")), Op::delete(p("a.txt"))]).unwrap();
        change.ops.reverse();
        assert_eq!(
            change.apply(&base),
            Err(ChangesetError::NotCanonical("a.txt".into()))
        );
        let dup = Changeset::new(
            &base,
            vec![Op::delete(p("link")), Op::put(p("link"), Entry::file(b"x"))],
        );
        assert_eq!(dup, Err(ChangesetError::NotCanonical("link".into())));
    }

    #[test]
    fn refuses_missing_deletes_and_no_op_puts() {
        let base = base();
        assert_eq!(
            Changeset::new(&base, vec![Op::delete(p("nope"))]),
            Err(ChangesetError::DeleteMissing("nope".into()))
        );
        assert_eq!(
            Changeset::new(&base, vec![Op::put(p("a.txt"), Entry::file(b"a"))]),
            Err(ChangesetError::NoOpPut("a.txt".into()))
        );
    }

    #[test]
    fn refuses_a_result_that_is_not_a_tree() {
        let err = Changeset::new(&base(), vec![Op::put(p("A.TXT"), Entry::file(b"x"))]);
        assert!(matches!(
            err,
            Err(ChangesetError::Tree(TreeError::Collision { .. }))
        ));
        let err = Changeset::new(&base(), vec![Op::put(p("a.txt/x"), Entry::file(b"x"))]);
        assert!(matches!(
            err,
            Err(ChangesetError::Tree(TreeError::FileDirConflict { .. }))
        ));
    }

    #[test]
    fn refuses_other_formats_and_unknown_fields() {
        let base = base();
        let mut change = Changeset::new(&base, vec![Op::delete(p("link"))]).unwrap();
        let json = serde_json::to_string(&change).unwrap();
        let extra = json.replacen('{', "{\"extra\":1,", 1);
        assert!(serde_json::from_str::<Changeset>(&extra).is_err());
        change.format = 2;
        assert_eq!(change.apply(&base), Err(ChangesetError::Format(2)));
    }

    #[test]
    fn json_round_trip() {
        let base = base();
        let change =
            Changeset::new(&base, vec![Op::put(p("ñ/日本.txt"), Entry::file(b"x"))]).unwrap();
        let json = serde_json::to_string(&change).unwrap();
        assert_eq!(serde_json::from_str::<Changeset>(&json).unwrap(), change);
    }
}
