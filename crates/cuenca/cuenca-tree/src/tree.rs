use crate::entry::{Entry, Mode};
use crate::error::{SymlinkProblem, TreeError};
use crate::oid::GitOid;
use crate::path::{RepoPath, fold_component, is_gitattributes_alias, is_gitmodules_alias};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;

use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// A validated tree. Every path is a [`RepoPath`], and:
///
/// - no path is both a file and a directory;
/// - no two names in one directory collide once case and Unicode normalisation are
///   folded, since a checkout on APFS could not hold both;
/// - no component is `.gitmodules` under any alias (the format has no submodules, and
///   git's `verify_path` and `fsck` refuse or reject such entries);
/// - a `.gitattributes` alias is only ever a regular or executable file: git's `fsck`
///   rejects a directory there (and only reports a symlink, which is refused here as
///   well); its content limits are checked by [`check_content`].
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

impl TreeObject {
    /// The object's bytes as git stores them: the canonical tree body.
    pub fn body(&self) -> Vec<u8> {
        tree_body(&self.children)
    }
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
        for (path, entry) in &self.entries {
            let count = path.components().count();
            for (i, name) in path.components().enumerate() {
                if is_gitmodules_alias(name) {
                    return Err(TreeError::Gitmodules(path.to_string()));
                }
                let is_dir = i + 1 < count;
                if is_gitattributes_alias(name) && (is_dir || entry.mode() == Mode::Symlink) {
                    return Err(TreeError::GitattributesNotAFile(path.to_string()));
                }
            }
            let full = path.as_str();
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

/// The largest `.gitattributes` git's fsck parses (`ATTR_MAX_FILE_SIZE`).
pub const MAX_GITATTRIBUTES_BYTES: usize = 100 * 1024 * 1024;
/// The longest `.gitattributes` line git's fsck parses, exclusive (`ATTR_MAX_LINE_LENGTH`).
pub const MAX_GITATTRIBUTES_LINE: usize = 2048;

/// The longest symlink target a Mac can check out: `symlink(2)` refuses a target of
/// `PATH_MAX` (1024) bytes or more.
pub const MAX_SYMLINK_TARGET: usize = 1023;

/// Whether [`check_content`] has a rule for content at `path` with `mode`, so the
/// content must be seen.
pub fn has_content_rule(path: &RepoPath, mode: Mode) -> bool {
    mode == Mode::Symlink || is_gitattributes_alias(path.file_name())
}

/// Check content at `path` with `mode` against the rules a tree of git ids cannot see:
///
/// - a symlink's target is non-empty, has no NUL byte and is at most
///   [`MAX_SYMLINK_TARGET`] bytes, since `symlink(2)` takes a NUL-terminated string and a
///   Mac refuses longer targets: otherwise no checkout holds the link as stored;
/// - a `.gitattributes` is at most 100 MiB with every line shorter than 2048 bytes
///   (git's `fsck`, which scans up to the first NUL).
///
/// The rules depend only on the content and the kind of path, so an acceptor checks
/// each blob id once per kind, not once per path naming it.
pub fn check_content(path: &RepoPath, mode: Mode, content: &[u8]) -> Result<(), TreeError> {
    if mode == Mode::Symlink {
        let problem = if content.is_empty() {
            Some(SymlinkProblem::Empty)
        } else if content.contains(&0) {
            Some(SymlinkProblem::Nul)
        } else if content.len() > MAX_SYMLINK_TARGET {
            Some(SymlinkProblem::TooLong(content.len()))
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(TreeError::SymlinkTarget {
                path: path.to_string(),
                problem,
            });
        }
    }
    if !is_gitattributes_alias(path.file_name()) {
        return Ok(());
    }
    let bad = |why: String| TreeError::GitattributesContent {
        path: path.to_string(),
        why,
    };
    if content.len() > MAX_GITATTRIBUTES_BYTES {
        return Err(bad(format!(
            "{} bytes, over {MAX_GITATTRIBUTES_BYTES}",
            content.len()
        )));
    }
    let scanned = content.split(|b| *b == 0).next().unwrap_or_default();
    if let Some(line) = scanned
        .split(|b| *b == b'\n')
        .find(|l| l.len() >= MAX_GITATTRIBUTES_LINE)
    {
        return Err(bad(format!(
            "a line of {} bytes, at or over {MAX_GITATTRIBUTES_LINE}",
            line.len()
        )));
    }
    Ok(())
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
    fn refuses_gitmodules_anywhere() {
        for (path, entry) in [
            ("sub/.GITMODULES", Entry::symlink(b"x")),
            (".gitmodules", Entry::file(b"x")),
            (".gitmodules/x", Entry::symlink(b"x")),
            ("a/.gitmodules/b", Entry::file(b"x")),
            ("gitmod~1:/x", Entry::symlink(b"x")),
            (".gitmodule\u{17F}", Entry::symlink(b"x")),
        ] {
            let t = Tree::from_entries([(p(path), entry)]);
            assert_eq!(t, Err(TreeError::Gitmodules(path.into())), "{path:?}");
        }
    }

    #[test]
    fn gitattributes_must_be_a_file() {
        assert!(Tree::from_entries([(p("a/.gitattributes"), Entry::file(b"* text"))]).is_ok());
        for (path, entry) in [
            (".gitattributes/x", Entry::file(b"x")),
            (".GITATTRIBUTES", Entry::symlink(b"x")),
            (".gitattribute\u{17F}/x", Entry::file(b"x")),
        ] {
            let t = Tree::from_entries([(p(path), entry)]);
            assert_eq!(
                t,
                Err(TreeError::GitattributesNotAFile(path.into())),
                "{path:?}"
            );
        }
    }

    #[test]
    fn gitattributes_content_limits() {
        let path = p("x/.gitattributes");
        assert!(check_content(&path, Mode::File, b"* text=auto\n").is_ok());
        assert!(check_content(&path, Mode::File, &vec![b'a'; MAX_GITATTRIBUTES_LINE - 1]).is_ok());
        assert!(check_content(&path, Mode::File, &vec![b'a'; MAX_GITATTRIBUTES_LINE]).is_err());
        let mut after_nul = b"ok\0".to_vec();
        after_nul.extend(vec![b'a'; 5000]);
        assert!(
            check_content(&path, Mode::File, &after_nul).is_ok(),
            "fsck stops at the first NUL"
        );
        assert!(check_content(&p("long.txt"), Mode::File, &vec![b'a'; 5000]).is_ok());
        let link = Mode::Symlink;
        assert!(check_content(&p("l"), link, &vec![b'a'; MAX_SYMLINK_TARGET]).is_ok());
        assert!(check_content(&p("l"), link, &vec![b'a'; MAX_SYMLINK_TARGET + 1]).is_err());
        assert!(check_content(&p("l"), link, b"").is_err());
        assert!(check_content(&p("l"), link, b"safe\0evil").is_err());
        assert!(check_content(&p("f"), Mode::File, b"").is_ok());
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
        let deep = vec!["a"; 512].join("/");
        let started = std::time::Instant::now();
        let t = tree(&[&deep]).unwrap();
        assert_eq!(t.git_objects().len(), 512);
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
