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
mutate src/ignore.rs 'NotFound => Ok(Self::defaults()),' 'NotFound => Self::parse(""),' --test fs forkignored
mutate src/scan.rs 'if ignore.is_ignored(&rel, kind.is_dir()) {' 'if false {' --test fs forkignored
# what a snapshot holds
mutate src/scan.rs 'if name == ".git" {' 'if false {' --test fs git_state
mutate src/scan.rs '                    skipped.push(Skipped {
                        path: abs,
                        reason: "git reserves the name",
                    });
                    continue;' '                    continue;' --test fs fails_loudly
mutate src/scan.rs '} else if kind.is_file() {' '} else if !kind.is_symlink() && !kind.is_dir() {' --test fs git_state
mutate src/scan.rs 'let entry = if stamp.is_executable() {' 'let entry = if false {' --test fs
mutate src/scan.rs 'if stamp.size() > MAX_FILE_BYTES {' 'if false {' --test fs blob_limit
mutate src/scan.rs 'check_content(&path, Mode::File, &content)?;' '' --test fs gitattributes
mutate src/scan.rs 'if known.insert(oid) {' 'if true {' --test fs new_blobs
# reading a file
mutate src/scan.rs '.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)' '.custom_flags(libc::O_NONBLOCK)' --lib
mutate src/scan.rs 'if after != stamp {' 'if false {' --lib
# the stat cache
mutate src/scan.rs 'match cache.lookup(path.as_str(), &stamp) {' 'match None::<Entry> {' --test fs stat_cache
mutate src/scan.rs 'next.record(found.path.to_string(), stamp, found.entry, started);' '' --test fs stat_cache
mutate src/scan.rs '    *cache = next;' '' --test fs stat_cache
mutate src/cache.rs '.filter(|c| c.stamp == *stamp)' '.filter(|_| true)' --test fs stat_cache
mutate src/cache.rs 'ctime: (meta.ctime(), meta.ctime_nsec()),' 'ctime: (0, 0),' --test fs stat_cache
mutate src/cache.rs 'let at = at.saturating_sub(RACY_MARGIN);' '' --test fs just_now
# checkout
mutate src/checkout.rs 'if items.next().is_some() {' 'if false {' --test fs non_empty
mutate src/checkout.rs 'verify(oid, &bytes)?;' '' --test fs wrong_or_missing
mutate src/checkout.rs 'verify(oid, &target)?;' '' --test fs wrong_or_missing
mutate src/checkout.rs 'if Stamp::of(&now) != stamp {' 'if false {' --test fs source_changed
mutate src/checkout.rs 'Ok(meta) if Stamp::of(&meta) == *stamp => {' 'Ok(_) => {' --test fs edited_since
mutate src/checkout.rs 'if stamp.permissions() != mode {' 'if false {' --test fs every_source
mutate src/checkout.rs 'if !stamp.settled(started) {' 'if false {' --test fs every_source

exit $survived
