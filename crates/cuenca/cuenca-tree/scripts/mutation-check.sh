#!/usr/bin/env bash
# Mutation check for cuenca-tree: each mutant breaks one guarantee, and the tests must
# fail for every one. Run from the crate directory: scripts/mutation-check.sh
# Prints one line per mutant and exits non-zero if any mutant survives. The source is
# restored after each mutant, also on interrupt.
set -uo pipefail
cd "$(dirname "$0")/.."
backup=$(mktemp -d)
restore() { [ -n "${current:-}" ] && cp "$backup/file" "$current"; current=; }
trap 'restore; rm -rf "$backup"' EXIT INT TERM

# A mutant only counts as killed against a suite that passes unmutated.
if ! cargo test -q -p cuenca-tree >"$backup/log" 2>&1; then
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
  if cargo test -q -p cuenca-tree "$@" >"$backup/log" 2>&1; then
    echo "SURVIVED  $file: $old"; survived=1
  else
    echo "killed    $file: $old"
  fi
  restore
}

mutate src/tree.rs "(mode == Mode::Tree).then_some(b'/')" "None::<u8>" --test round_trip
mutate src/entry.rs 'Self::Tree => "40000",' 'Self::Tree => "040000",' --test round_trip
mutate src/path.rs 'UniCase::unicode(decomposed)
        .to_folded_case()' 'decomposed
        .to_lowercase()' --lib
mutate src/path.rs "        if c == b':' {
            return true;" "        if c == b':' {
            return false;" --lib
mutate src/tree.rs 'if entry.mode() == Mode::Symlink && is_gitmodules_alias(path.file_name())' 'if false' --lib
mutate src/tree.rs 'Slot::Occupied(seen) if *seen.get() != (blob.sha256, blob.size) =>' 'Slot::Occupied(seen) if false && *seen.get() != (blob.sha256, blob.size) =>' --lib
mutate src/changeset.rs 'if actual != self.target {' 'if false {' --lib
mutate src/changeset.rs '.is_some_and(|claimed| *claimed != entry.blob())' '.is_some_and(|claimed| false && *claimed != entry.blob())' --lib
mutate src/git.rs '.env("GIT_NO_REPLACE_OBJECTS", "1")' '' --test git_bridge
mutate src/git.rs '.env("GIT_NO_LAZY_FETCH", "1")' '' --test git_bridge
mutate src/git.rs 'if hash_stream(oid, size, content)? != claims[&oid] {' 'if false {' --test git_bridge
mutate src/git.rs 'if GitOid::for_blob(&bytes) != oid {' 'if false {' --test git_bridge
mutate src/git.rs 'if let Some(empty) = dirs.iter().find(|d| !has_files.contains(d.as_str()))' 'if let Some(empty) = dirs.iter().find(|_| false)' --test git_bridge

exit $survived
