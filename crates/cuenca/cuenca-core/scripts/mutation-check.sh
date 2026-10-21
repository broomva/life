#!/usr/bin/env bash
# Mutation check for cuenca-core: each mutant breaks one guarantee, and the tests must
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
if ! cargo test -q -p cuenca-core >"$backup/log" 2>&1; then
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
  if cargo test -q -p cuenca-core "$@" >"$backup/log" 2>&1; then
    echo "SURVIVED  $file: $old"; survived=1
  elif grep -qE '^error\[E[0-9]+\]|could not compile|failed to select a version' "$backup/log"; then
    echo "NO BUILD  $file: $old (a mutant that does not build proves nothing)"; survived=1
  else
    echo "killed    $file: $old"
  fi
  restore
}

# Blob ingest: only bytes that hash to the claim, of exactly the announced size.
mutate src/blobs.rs 'if blob.oid() != claimed {' 'if false {' --test store
mutate src/blobs.rs 'content.take(size + 1)' 'content.take(size)' --test store
mutate src/blobs.rs 'if size > MAX_BLOB_BYTES {' 'if false {' --test store
mutate src/blobs.rs 'Some(blob) if blob.oid() == oid => Ok(bytes),' 'Some(_) => Ok(bytes),' --test store
# Entries enter the log only with their blob, and content rules read the blob.
mutate src/store.rs 'if blobs.get(oid.as_bytes().as_slice())?.is_none() {
            return Err(CoreError::MissingBlob(oid));' 'if false {
            return Err(CoreError::MissingBlob(oid));' --test store
mutate src/store.rs 'check_content(path, entry.mode(), &self.blobs.read(oid)?)?;' '' --test store
# Publishing: from the head only; a retry appends nothing.
mutate src/store.rs '&& head == id' '&& false' --test store
mutate src/store.rs 'if changeset.base != state.head {
                return Err(CoreError::NotAtBase {' 'if false {
                return Err(CoreError::NotAtBase {' --test store
# Main moves by compare-and-swap; a retry appends nothing; intents name the head.
mutate src/store.rs 'if changeset.base != loc.main {' 'if false {' --test store
mutate src/store.rs 'if loc.main_changeset == Some(id) {' 'if false {' --test store
mutate src/store.rs 'if state.head_changeset.map(|(head, _)| head) != Some(id) {' 'if false {' --test store
# Names and remotes.
mutate src/names.rs '&& !s.contains("..")' '' --lib
mutate src/names.rs '&& b[0].is_ascii_alphanumeric()' '' --lib
mutate src/store.rs "&& !remote.starts_with('-')" '' --test store
# Trees are rebuilt and checked against their id.
mutate src/store.rs 'if tree.git_tree_id() != root {' 'if false {' --lib
# Replay finds every way the log or the tables can be wrong.
mutate src/store.rs 'if offset != records {' 'if false {' --lib
mutate src/store.rs 'if ChangesetId::of(&changeset) != id {' 'if false {' --lib
mutate src/store.rs 'if state.head_changeset.map(|(h, _)| h) == Some(id) {' 'if false {' --lib
mutate src/store.rs 'if table_forks != forks {' 'if false {' --lib
mutate src/store.rs '.map_or(0, |last| last + 1)' '.map_or(0, |last| last + 2)' --test store
# Commits are durable before they return: without it, a kill loses acknowledged drafts.
mutate src/store.rs 'txn.set_durability(Durability::Immediate);' 'txn.set_durability(Durability::None);' --test crash

exit $survived
