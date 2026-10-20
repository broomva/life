use crate::entry::{Entry, Mode};
use crate::error::TreeError;
use crate::oid::GitOid;
use crate::path::{RepoPath, fold_key};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

/// A validated tree: every path is a [`RepoPath`], no path is both a file and a
/// directory, and no two names collide on a case- or normalisation-insensitive
/// filesystem. Directories are implicit, so an empty directory cannot exist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<RepoPath, Entry>",
    into = "BTreeMap<RepoPath, Entry>"
)]
pub struct Tree {
    entries: BTreeMap<RepoPath, Entry>,
}

/// One git tree object, as [`Tree::git_objects`] builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeObject {
    /// The directory, `""` for the root.
    pub dir: String,
    pub oid: GitOid,
    /// The children as `(mode, name, oid)`, in git's order.
    pub children: Vec<(Mode, String, GitOid)>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a tree, refusing duplicate paths and anything [`Tree::validate`] refuses.
    pub fn from_entries(
        entries: impl IntoIterator<Item = (RepoPath, Entry)>,
    ) -> Result<Self, TreeError> {
        let mut map = BTreeMap::new();
        for (path, entry) in entries {
            if map.contains_key(&path) {
                return Err(TreeError::Duplicate(path.to_string()));
            }
            map.insert(path, entry);
        }
        Self::from_map(map)
    }

    pub(crate) fn from_map(entries: BTreeMap<RepoPath, Entry>) -> Result<Self, TreeError> {
        let tree = Self { entries };
        tree.validate()?;
        Ok(tree)
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.get(path)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&RepoPath, &Entry)> {
        self.entries.iter()
    }

    pub(crate) fn map(&self) -> &BTreeMap<RepoPath, Entry> {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Check that no path is both a file and a directory, and that no two names (files
    /// or directories) collide once case and Unicode normalisation are folded, since a
    /// checkout on APFS could not hold both.
    pub fn validate(&self) -> Result<(), TreeError> {
        let mut folded: HashMap<String, &str> = HashMap::new();
        for path in self.entries.keys() {
            for dir in path.ancestors() {
                if self.entries.contains_key(dir) {
                    return Err(TreeError::FileDirConflict {
                        file: dir.to_string(),
                        inside: path.to_string(),
                    });
                }
            }
            for name in path.ancestors().chain(std::iter::once(path.as_str())) {
                match folded.get(&fold_key(name)) {
                    Some(seen) if *seen != name => {
                        return Err(TreeError::Collision {
                            a: seen.to_string(),
                            b: name.to_string(),
                        });
                    }
                    Some(_) => {}
                    None => {
                        folded.insert(fold_key(name), name);
                    }
                }
            }
        }
        Ok(())
    }

    /// The git tree id of this tree, computed in-process.
    pub fn git_tree_id(&self) -> GitOid {
        self.git_objects()
            .last()
            .map(|t| t.oid)
            .unwrap_or_else(GitOid::empty_tree)
    }

    /// Every git tree object of this tree, each directory after the directories inside
    /// it, so the root comes last.
    pub fn git_objects(&self) -> Vec<TreeObject> {
        let mut dirs: BTreeMap<String, Vec<(Mode, String, GitOid)>> = BTreeMap::new();
        dirs.insert(String::new(), Vec::new());
        for (path, entry) in &self.entries {
            for dir in path.ancestors() {
                dirs.entry(dir.to_string()).or_default();
            }
            let (dir, name) = path.split_last();
            dirs.get_mut(dir).expect("ancestors were inserted").push((
                entry.mode(),
                name.to_string(),
                entry.oid(),
            ));
        }

        let mut order: Vec<String> = dirs.keys().cloned().collect();
        order.sort_by_key(|d| std::cmp::Reverse(depth(d)));

        let mut objects = Vec::with_capacity(order.len());
        for dir in order {
            let mut children = dirs.remove(&dir).expect("each directory is visited once");
            children.sort_by(git_name_cmp);
            let oid = GitOid::for_object("tree", &tree_body(&children));
            if !dir.is_empty() {
                let (parent, name) = match dir.rfind('/') {
                    Some(i) => (&dir[..i], &dir[i + 1..]),
                    None => ("", dir.as_str()),
                };
                dirs.get_mut(parent)
                    .expect("a parent is shallower, so not yet visited")
                    .push((Mode::Tree, name.to_string(), oid));
            }
            objects.push(TreeObject { dir, oid, children });
        }
        objects
    }
}

fn depth(dir: &str) -> usize {
    if dir.is_empty() {
        0
    } else {
        dir.matches('/').count() + 1
    }
}

/// Git's order for entries of one tree: by name bytes, where a tree's name is compared
/// as if it ended in `/` (git's `base_name_compare`).
fn git_name_cmp(a: &(Mode, String, GitOid), b: &(Mode, String, GitOid)) -> Ordering {
    let (an, bn) = (a.1.as_bytes(), b.1.as_bytes());
    let common = an.len().min(bn.len());
    match an[..common].cmp(&bn[..common]) {
        Ordering::Equal => {}
        other => return other,
    }
    let tail = |name: &[u8], mode: Mode| {
        name.get(common)
            .copied()
            .or((mode == Mode::Tree).then_some(b'/'))
    };
    tail(an, a.0).cmp(&tail(bn, b.0))
}

fn tree_body(children: &[(Mode, String, GitOid)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (mode, name, oid) in children {
        body.extend_from_slice(mode.as_str().as_bytes());
        body.push(b' ');
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(oid.as_bytes());
    }
    body
}

impl TryFrom<BTreeMap<RepoPath, Entry>> for Tree {
    type Error = TreeError;
    fn try_from(entries: BTreeMap<RepoPath, Entry>) -> Result<Self, TreeError> {
        Self::from_map(entries)
    }
}

impl From<Tree> for BTreeMap<RepoPath, Entry> {
    fn from(tree: Tree) -> Self {
        tree.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RepoPath {
        RepoPath::new(s).unwrap()
    }

    fn tree(paths: &[&str]) -> Result<Tree, TreeError> {
        Tree::from_entries(paths.iter().map(|s| (p(s), Entry::file(s.as_bytes()))))
    }

    #[test]
    fn empty_tree_is_gits_empty_tree() {
        assert_eq!(Tree::new().git_tree_id(), GitOid::empty_tree());
    }

    #[test]
    fn rejects_file_that_is_also_a_directory() {
        assert_eq!(
            tree(&["a", "a/b"]),
            Err(TreeError::FileDirConflict {
                file: "a".into(),
                inside: "a/b".into()
            })
        );
    }

    #[test]
    fn rejects_case_and_normalisation_collisions() {
        for pair in [
            ["README.md", "readme.md"],
            ["Src/a", "src/b"],
            ["A", "a/x"],
            ["caf\u{e9}.txt", "cafe\u{301}.txt"],
            ["d/\u{c9}/x", "d/e\u{301}/y"],
        ] {
            assert!(
                matches!(tree(&pair), Err(TreeError::Collision { .. })),
                "{pair:?}"
            );
        }
    }

    #[test]
    fn rejects_duplicates() {
        let e = Entry::file(b"x");
        assert_eq!(
            Tree::from_entries([(p("a"), e), (p("a"), e)]),
            Err(TreeError::Duplicate("a".into()))
        );
    }

    #[test]
    fn git_order_puts_a_tree_after_names_with_lower_bytes_than_slash() {
        // "x-y" ('-' 0x2d) < "x.y" ('.' 0x2e) < "x/" (tree, '/' 0x2f) < "x0" ('0' 0x30)
        let t = tree(&["x0", "x.y", "x/inner", "x-y"]).unwrap();
        let root = t.git_objects().pop().unwrap();
        let names: Vec<&str> = root.children.iter().map(|c| c.1.as_str()).collect();
        assert_eq!(names, ["x-y", "x.y", "x", "x0"]);
    }

    #[test]
    fn serde_revalidates() {
        let t = tree(&["a/b", "c"]).unwrap();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Tree>(&json).unwrap(), t);
        let bad = json.replace("\"c\"", "\"A/B\"");
        assert!(
            serde_json::from_str::<Tree>(&bad).is_err(),
            "collision must not deserialize"
        );
    }
}
