//! Read a real repository's tree through the bridge and check that cuenca's tree id is
//! git's: `cargo run --release -p cuenca-tree --example verify -- <repo> [treeish]`.

use cuenca_tree::Entry;
use cuenca_tree::git::Repo;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: verify <repo> [treeish]");
        return ExitCode::from(2);
    };
    let treeish = args.next().unwrap_or_else(|| "HEAD".to_string());
    let repo = Repo::open(&path);
    let started = Instant::now();
    let result = repo
        .resolve_tree(&treeish)
        .and_then(|git| Ok((git, repo.read_tree(&treeish)?)));
    let (git, tree) = match result {
        Ok(found) => found,
        Err(e) => {
            eprintln!("{path} {treeish}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (mut files, mut execs, mut links, mut gitlinks, mut bytes) = (0, 0, 0, 0, 0u64);
    for (_, entry) in tree.entries() {
        match entry {
            Entry::File(b) => (files, bytes) = (files + 1, bytes + b.size),
            Entry::Executable(b) => (execs, bytes) = (execs + 1, bytes + b.size),
            Entry::Symlink(b) => (links, bytes) = (links + 1, bytes + b.size),
            Entry::Gitlink(_) => gitlinks += 1,
        }
    }
    let ours = tree.git_tree_id();
    println!(
        "{path} {treeish}: {} entries ({files} files, {execs} executables, {links} symlinks, {gitlinks} gitlinks), {:.1} MiB, {:.2} s",
        tree.len(),
        bytes as f64 / 1_048_576.0,
        started.elapsed().as_secs_f64()
    );
    println!("git    {git}\ncuenca {ours}");
    if ours == git {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
