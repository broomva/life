#!/usr/bin/env bash
# Mutation check for cuenca-fs: each mutant breaks one guarantee, and the tests must
# fail for every one. Run from the crate directory: scripts/mutation-check.sh
# Prints one line per mutant and exits non-zero if any mutant survives. The source is
# restored after each mutant, also on interrupt.
set -uo pipefail
cd "$(dirname "$0")/.."
backup=$(mktemp -d)
lock=$(git rev-parse --show-toplevel)/Cargo.lock
cp "$lock" "$backup/Cargo.lock"
restore() { [ -n "${current:-}" ] && cp "$backup/file" "$current"; current=; cp "$backup/Cargo.lock" "$lock"; }
trap 'restore; rm -rf "$backup"' EXIT
trap 'exit 130' INT TERM

# A mutant only counts as killed against a suite that passes unmutated.
if ! cargo test -q -p cuenca-fs >"$backup/log" 2>&1; then
  echo "baseline is red: fix the suite before reading any mutant verdict"; cat "$backup/log"; exit 2
fi
echo "baseline  green"

survived=0
mutate() { # file, old, new, then the cargo test arguments that should catch it
  local file=$1 old=$2 new=$3; shift 3
  current=$file; cp "$file" "$backup/file"
  if ! python3 - "$file" "$old" "$new" <<'PY'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
if s.count(sys.argv[2]) != 1: sys.exit("anchor not unique: " + sys.argv[2])
p.write_text(s.replace(sys.argv[2], sys.argv[3]))
PY
  then echo "ANCHOR MISSING  $file: $old"; survived=1; restore; return; fi
  if cargo test -q -p cuenca-fs "$@" >"$backup/log" 2>&1; then
    echo "SURVIVED  $file: $old"; survived=1
  elif grep -qE '^error\[E[0-9]+\]|could not compile|failed to select a version' "$backup/log"; then
    echo "NO BUILD  $file: $old (a mutant that does not build proves nothing)"; survived=1
  else
    echo "killed    $file: $old"
  fi
  restore
}

# .forkignore
mutate src/ignore.rs 'Errno::NOENT) => return Ok((Self::defaults(), None)),' 'Errno::NOENT) => return Ok((Self::parse("")?, None)),' --test fs forkignored
mutate src/ignore.rs 'let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;' 'let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;' --lib symlink_or_a_directory
mutate src/ignore.rs 'if !file.metadata().map_err(FsError::io(&path))?.is_file() {' 'if false {' --lib symlink_or_a_directory
mutate src/ignore.rs 'if self.is_ignored(&path[..end], true) {' 'if false {' --lib excludes
mutate src/scan.rs 'let ignored = tracked_only || ignore.is_ignored(&rel, is_dir);' 'let ignored = tracked_only;' --test fs forkignore
mutate src/scan.rs 'if held != parsed.map(GitOid::for_blob) {' 'if false {' --lib rules_that_changed
mutate src/scan.rs 'if after.as_deref() != parsed {' 'if false {' --lib rules_changed_after
# tracked paths under the rules (git's rule)
mutate src/scan.rs 'let ignored = tracked_only || ignore.is_ignored(&rel, is_dir);' 'let ignored = ignore.is_ignored(&rel, is_dir);' --test fs stay_tracked
mutate src/scan.rs 'tracked_dirs.contains(rel_str.as_str())' 'true' --test fs never_entered
mutate src/scan.rs 'base.get(&rel_str).is_some()' 'true' --test fs stay_tracked
mutate src/scan.rs '                if !tracked {
                    continue;
                }' '' --test fs stay_tracked
# what a snapshot holds
mutate src/scan.rs 'if bytes == b"." || bytes == b".." || bytes == b".git" {' 'if bytes == b"." || bytes == b".." {' --test fs git_state
mutate src/scan.rs '                    skipped.push(Skipped {
                        path: rel_str,
                        reason: "git reserves the name",
                    });
                    continue;' '                    continue;' --test fs fails_loudly
mutate src/scan.rs 'FileType::RegularFile => {' 'FileType::RegularFile | FileType::Fifo | FileType::Socket => {' --test fs git_state
mutate src/scan.rs 'let entry = if stamp.is_executable() {' 'let entry = if false {' --test fs
mutate src/scan.rs 'if stamp.size() > MAX_FILE_BYTES {' 'if false {' --test fs blob_limit
mutate src/scan.rs 'check_content(&path, Mode::File, &content)?;' '' --test fs gitattributes
mutate src/scan.rs 'if !known.insert(oid) {' 'if false {' --test fs new_blobs
mutate src/scan.rs '            Err(Errno::LOOP) => None,
            Err(Errno::NOENT) => return Ok(None),' '            Err(Errno::LOOP) => None,' --lib removed_after_the_listing
# never leaving the root
mutate src/scan.rs 'let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;' 'let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;' --lib renamed_out_of_the_root
mutate src/scan.rs 'if dev_ino(&fstat(&fd).map_err(errno(&p.abs))?) != p.dev_ino {' 'if false {' --lib swapped_for_a_symlink
mutate src/scan.rs 'let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;' 'let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;' --lib replaced_by_a_symlink
mutate src/scan.rs 'if after != stamp {' 'if false {' --lib stale_stamp
# the stat cache
mutate src/scan.rs 'match cache.lookup(path.as_str(), &stamp) {' 'match None::<Entry> {' --test fs stat_cache
mutate src/scan.rs 'next.record(f.path.to_string(), stamp, f.entry, started);' '' --test fs stat_cache
mutate src/scan.rs '    rules_unchanged(root, parsed.as_deref())?;
    *cache = next;' '    rules_unchanged(root, parsed.as_deref())?;' --test fs stat_cache
mutate src/cache.rs '.filter(|c| c.stamp == *stamp)' '.filter(|_| true)' --test fs stat_cache
mutate src/cache.rs 'ctime: (meta.ctime(), meta.ctime_nsec()),' 'ctime: (0, 0),' --lib raw_stat
mutate src/cache.rs 'ctime: (st.st_ctime as i64, st.st_ctime_nsec as i64),' 'ctime: (0, 0),' --test fs stat_cache
mutate src/cache.rs 'let at = at.saturating_sub(RACY_MARGIN);' '' --test fs just_now
# checkout
mutate src/checkout.rs '    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = match openat(CWD, dest, flags' '    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let fd = match openat(CWD, dest, flags' --test fs link_or_a_file
mutate src/checkout.rs '    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut at' '    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let mut at' --lib inside_the_destination
mutate src/checkout.rs 'if !matches!(item.file_name().to_bytes(), b"." | b"..") {' 'if false {' --test fs non_empty
mutate src/checkout.rs 'verify(oid, &bytes)?;' '' --test fs wrong_or_missing
mutate src/checkout.rs 'verify(oid, &target)?;' '' --test fs wrong_or_missing
mutate src/checkout.rs 'if now != stamp {' 'if false {' --test fs source_changed
mutate src/checkout.rs '(cloned, stamp.is_some())' '(cloned, true)' --test fs corrupt_object
mutate src/checkout.rs 'cache.record(w.path, w.stamp, w.entry, started);' 'cache.record(w.path, w.stamp, w.entry, started + std::time::Duration::from_secs(3600));' --test fs every_source
mutate src/checkout.rs '&& Stamp::of(&file.metadata()?) == *stamp' '&& true' --test fs edited_since
mutate src/checkout.rs 'if Blob::of(&content).map(|b| b.oid()) == Some(oid) {' 'if true {' --test fs edited_since
mutate src/checkout.rs 'if stamp.permissions() != mode {' 'if false {' --test fs every_source

exit $survived
