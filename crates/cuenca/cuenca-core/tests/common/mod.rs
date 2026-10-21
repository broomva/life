#![allow(dead_code)]

use cuenca_core::{ForkName, Holder, LocationName, NodeId, PrincipalName, Store, Writer};
use cuenca_tree::{Blob, Changeset, Entry, GitOid, Op, RepoPath, Tree};

pub fn loc(s: &str) -> LocationName {
    LocationName::new(s).unwrap()
}

pub fn fork(s: &str) -> ForkName {
    ForkName::new(s).unwrap()
}

pub fn path(s: &str) -> RepoPath {
    RepoPath::new(s).unwrap()
}

/// Ingest `content` and return its git id.
pub fn ingest(store: &Store, content: &[u8]) -> GitOid {
    let oid = GitOid::for_blob(content);
    store
        .ingest_blob(oid, content.len() as u64, content)
        .unwrap();
    oid
}

/// A tree of regular files, ingesting each one's content.
pub fn tree(store: &Store, files: &[(&str, &[u8])]) -> Tree {
    Tree::from_entries(
        files
            .iter()
            .map(|(p, c)| (path(p), Entry::File(ingest(store, c)))),
    )
    .unwrap()
}

/// The changeset that writes `files` over `base`, ingesting their content.
pub fn write(store: &Store, base: &Tree, files: &[(&str, &[u8])]) -> Changeset {
    let ops = files
        .iter()
        .map(|(p, c)| Op::put(path(p), Entry::File(ingest(store, c))))
        .collect();
    Changeset::new(base, ops).unwrap()
}

pub fn sha256(content: &[u8]) -> cuenca_tree::ContentHash {
    Blob::of(content).unwrap().sha256()
}

pub fn holder(principal: &str, node: &str) -> Holder {
    Holder::new(
        PrincipalName::new(principal).unwrap(),
        NodeId::new(node).unwrap(),
    )
}

/// The holder the tests create forks with.
pub fn agent() -> Holder {
    holder("agent", "nAgentCNTRL")
}

/// [`agent`] under epoch 1, a new fork's.
pub fn w1() -> Writer {
    Writer::new(agent(), 1)
}
