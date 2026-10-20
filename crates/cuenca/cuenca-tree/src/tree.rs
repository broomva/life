use crate::entry::{Entry, Mode};
use crate::error::TreeError;
use crate::oid::{ContentHash, GitOid};
use crate::path::{RepoPath, fold_component, is_gitmodules_alias};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::collections::hash_map::Entry as Slot;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// A validated tree. Every path is a [`RepoPath`], and:
///
/// - no path is both a file and a directory;
/// - no two names in one directory collide once case and Unicode normalisation are
///   folded, since a checkout on APFS could not hold both;
/// - no symlink is named `.gitmodules` under any alias git refuses;
/// - each git blob id is claimed with one SHA-256 and size.
///
/// Directories are implicit, so an empty directory cannot exist. Validation and
/// [`Tree::git_objects`] are linear in the total length of the paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    entries: BTreeMap<RepoPath, Entry>,
}

/// One git tree object, as [`Tree::git_objects`] builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeObject {
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

    /// Check the invariants listed on [`Tree`].
    pub fn validate(&self) -> Result<(), TreeError> {
        struct Seen<'a> {
            id: usize,
            name: &'a str,
            prefix: &'a str,
            is_file: bool,
            first_path: &'a str,
        }
        // Each directory or file is a node, keyed by its parent node and its folded name.
        let mut nodes: HashMap<(usize, String), Seen> = HashMap::new();
        let mut next_id = 1;
        let mut blobs: HashMap<GitOid, (ContentHash, u64)> = HashMap::new();

        for (path, entry) in &self.entries {
            let blob = entry.blob();
            match blobs.entry(blob.oid) {
                Slot::Occupied(seen) if *seen.get() != (blob.sha256, blob.size) => {
                    return Err(TreeError::InconsistentBlob(blob.oid));
                }
                Slot::Occupied(_) => {}
                Slot::Vacant(slot) => {
                    slot.insert((blob.sha256, blob.size));
                }
            }
            if entry.mode() == Mode::Symlink && is_gitmodules_alias(path.file_name()) {
                return Err(TreeError::SymlinkedGitmodules(path.to_string()));
            }

            let full = path.as_str();
            let count = path.components().count();
            let (mut parent, mut start) = (0, 0);
            for (i, name) in path.components().enumerate() {
                let end = start + name.len();
                let prefix = &full[..end];
                let is_file = i + 1 == count;
                match nodes.get(&(parent, fold_component(name))) {
                    Some(seen) if seen.name != name => {
                        return Err(TreeError::Collision {
                            a: seen.prefix.to_string(),
                            b: prefix.to_string(),
                        });
                    }
                    Some(seen) if seen.is_file => {
                        return Err(TreeError::FileDirConflict {
                            file: seen.prefix.to_string(),
                            inside: full.to_string(),
                        });
                    }
                    Some(seen) if is_file => {
                        return Err(TreeError::FileDirConflict {
                            file: full.to_string(),
                            inside: seen.first_path.to_string(),
                        });
                    }
                    Some(seen) => parent = seen.id,
                    None => {
                        nodes.insert(
                            (parent, fold_component(name)),
                            Seen {
                                id: next_id,
                                name,
                                prefix,
                                is_file,
                                first_path: full,
                            },
                        );
                        parent = next_id;
                        next_id += 1;
                    }
                }
                start = end + 1;
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
        struct Dir<'a> {
            name: &'a str,
            parent: usize,
            files: Vec<(Mode, &'a str, GitOid)>,
            subdirs: HashMap<&'a str, usize>,
        }
        let mut arena = vec![Dir {
            name: "",
            parent: 0,
            files: Vec::new(),
            subdirs: HashMap::new(),
        }];
        for (path, entry) in &self.entries {
            let (dir, file) = path.split_last();
            let mut at = 0;
            if !dir.is_empty() {
                for name in dir.split('/') {
                    at = match arena[at].subdirs.get(name) {
                        Some(&child) => child,
                        None => {
                            let child = arena.len();
                            arena.push(Dir {
                                name,
                                parent: at,
                                files: Vec::new(),
                                subdirs: HashMap::new(),
                            });
                            arena[at].subdirs.insert(name, child);
                            child
                        }
                    };
                }
            }
            arena[at].files.push((entry.mode(), file, entry.oid()));
        }

        // A directory is always created after its parent, so walking the arena
        // backwards visits every directory after the directories inside it.
        let mut child_trees: Vec<Vec<(Mode, String, GitOid)>> = vec![Vec::new(); arena.len()];
        let mut objects = Vec::with_capacity(arena.len());
        for idx in (0..arena.len()).rev() {
            let dir = &arena[idx];
            let mut children = std::mem::take(&mut child_trees[idx]);
            children.extend(dir.files.iter().map(|(m, n, o)| (*m, n.to_string(), *o)));
            children.sort_by(git_name_cmp);
            let oid = GitOid::for_object("tree", &tree_body(&children));
            if idx != 0 {
                child_trees[dir.parent].push((Mode::Tree, dir.name.to_string(), oid));
            }
            objects.push(TreeObject { oid, children });
        }
        objects
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

impl Serialize for Tree {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.entries.len()))?;
        for (path, entry) in &self.entries {
            map.serialize_entry(path, entry)?;
        }
        map.end()
    }
}

/// Deserializing refuses a path given twice (JSON would otherwise keep the last) and
/// re-validates the whole tree.
impl<'de> Deserialize<'de> for Tree {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct TreeVisitor;
        impl<'de> Visitor<'de> for TreeVisitor {
            type Value = Tree;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map from repository paths to entries")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Tree, A::Error> {
                let mut entries = BTreeMap::new();
                while let Some((path, entry)) = access.next_entry::<RepoPath, Entry>()? {
                    if entries.contains_key(&path) {
                        return Err(serde::de::Error::custom(TreeError::Duplicate(
                            path.to_string(),
                        )));
                    }
                    entries.insert(path, entry);
                }
                Tree::from_map(entries).map_err(serde::de::Error::custom)
            }
        }
        d.deserialize_map(TreeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Blob;

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
            ["straße", "strasse"],
            ["ſ/x", "s"],
            ["ﬁle", "file"],
            ["aς", "aσ"],
            ["µ", "μ"],
        ] {
            assert!(
                matches!(tree(&pair), Err(TreeError::Collision { .. })),
                "{pair:?}"
            );
        }
        assert!(
            tree(&["a/x", "b/x", "a/y"]).is_ok(),
            "the same name in two directories is fine"
        );
    }

    #[test]
    fn rejects_symlinked_gitmodules() {
        let t = Tree::from_entries([(p("sub/.GITMODULES"), Entry::symlink(b"x"))]);
        assert_eq!(
            t,
            Err(TreeError::SymlinkedGitmodules("sub/.GITMODULES".into()))
        );
        assert!(Tree::from_entries([(p(".gitmodules"), Entry::file(b"x"))]).is_ok());
    }

    #[test]
    fn rejects_one_oid_with_two_claims() {
        let real = Blob::of(b"real");
        let forged = Blob {
            sha256: Blob::of(b"evil").sha256,
            ..real
        };
        let t = Tree::from_entries([(p("a"), Entry::File(real)), (p("b"), Entry::File(forged))]);
        assert_eq!(t, Err(TreeError::InconsistentBlob(real.oid)));
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
    fn deepest_allowed_path_validates_fast() {
        let deep = vec!["a"; 2048].join("/");
        let started = std::time::Instant::now();
        let t = tree(&[&deep]).unwrap();
        assert_eq!(t.git_objects().len(), 2048);
        assert!(
            started.elapsed().as_secs() < 2,
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn serde_revalidates_and_refuses_duplicate_keys() {
        let t = tree(&["a/b", "c"]).unwrap();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Tree>(&json).unwrap(), t);
        let bad = json.replace("\"c\"", "\"A/B\"");
        assert!(
            serde_json::from_str::<Tree>(&bad).is_err(),
            "collision must not deserialize"
        );
        let e = serde_json::to_string(&Entry::file(b"x")).unwrap();
        let dup = format!("{{\"a\":{e},\"a\":{e}}}");
        assert!(
            serde_json::from_str::<Tree>(&dup).is_err(),
            "a duplicate key must not deserialize"
        );
    }
}
