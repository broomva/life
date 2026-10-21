//! Measure spec row 2·B "fork of a tree already checked out": snapshot `<src>` (twice, so
//! the second scan runs from the stat cache), check its tree out into `<dest>` from the
//! source checkout, and snapshot the result (a checkout leaves no cache, so that first
//! snapshot reads every file, and the one after it none).
//!
//! cargo run --release -p cuenca-fs --example fork_bench -- <src> <dest>
//!
//! The disk figure is the change in the used blocks of the filesystem holding `<dest>`,
//! so run it on a volume nothing else writes to (a fresh APFS sparse image).

use cuenca_fs::{CheckoutSource, StatCache, checkout, snapshot};
use cuenca_tree::Tree;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [src, dest] = args.as_slice() else {
        return Err("usage: fork_bench <src> <dest>".into());
    };
    let mut cache = StatCache::new();
    let t = Instant::now();
    let snap = snapshot(src, &Tree::new(), &mut cache)?;
    println!(
        "snapshot (cold)  {:>8.3}s  {} entries, {} hashed",
        t.elapsed().as_secs_f64(),
        snap.tree.len(),
        snap.hashed
    );
    let t = Instant::now();
    let warm = snapshot(src, &snap.tree, &mut cache)?;
    println!(
        "snapshot (warm)  {:>8.3}s  {} hashed, clean={}",
        t.elapsed().as_secs_f64(),
        warm.hashed,
        warm.is_clean()
    );
    let used = || -> std::io::Result<u64> {
        let parent = dest.parent().unwrap_or(dest);
        let st = rustix::fs::statvfs(parent)?;
        Ok((st.f_blocks - st.f_bfree) * st.f_frsize)
    };
    let tree_bytes: u64 = snap.new_blobs.iter().map(|b| b.size).sum();
    let before = used()?;
    let t = Instant::now();
    let done = checkout(
        &snap.tree,
        dest,
        &CheckoutSource::new(src, &snap.tree, &cache),
    )?;
    println!(
        "checkout         {:>8.3}s  {} cloned, {} copied",
        t.elapsed().as_secs_f64(),
        done.cloned,
        done.copied
    );
    let grew = used()?.saturating_sub(before);
    println!(
        "disk             {:>8} KiB more for {} KiB of content ({:.2}%)",
        grew / 1024,
        tree_bytes / 1024,
        100.0 * grew as f64 / tree_bytes as f64
    );
    let mut fork_cache = StatCache::new();
    let t = Instant::now();
    let fork = snapshot(dest, &snap.tree, &mut fork_cache)?;
    println!(
        "snapshot (fork)  {:>8.3}s  {} hashed, clean={}, same tree={}",
        t.elapsed().as_secs_f64(),
        fork.hashed,
        fork.is_clean(),
        fork.tree.git_tree_id() == snap.tree.git_tree_id()
    );
    Ok(())
}
