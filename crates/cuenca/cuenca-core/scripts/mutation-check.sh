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
mutate src/store.rs 'if self.blob_info(oid)?.is_none() {
            return Err(CoreError::MissingBlob(oid));
        }
        self.blobs.read(oid)' 'self.blobs.read(oid)' --test store
# Entries enter the log only with their blob, and content rules read the blob.
mutate src/store.rs 'if self.blob_info(oid)?.is_none() {
                return Err(CoreError::MissingBlob(oid));
            }
            if has_content_rule' 'if has_content_rule' --test store
mutate src/store.rs 'check_content(path, entry.mode(), &self.blobs.read(oid)?)?;' '' --test store
# A draft's ops and target are checked against the base tree, and the target stored.
mutate src/store.rs 'changeset.apply(&load_tree(&trees, changeset.base)?)?' 'load_tree(&trees, changeset.base)?' --test store
mutate src/store.rs 'let target = changeset.apply(&load_tree(&trees, changeset.base)?)?;
            store_tree(&mut trees, &target)' 'match changeset.apply(&load_tree(&trees, changeset.base)?) {
                Ok(target) => store_tree(&mut trees, &target),
                Err(_) => Ok(()),
            }' --test store
# The transition rules (one copy, used by the writer and by replay).
mutate src/state.rs 'if view.location(name)?.is_some() {' 'if false {' --test store
mutate src/state.rs 'if view.fork(name)?.is_some() {' 'if false {' --test store
mutate src/state.rs 'if *base != loc.main {' 'if false {' --lib
mutate src/state.rs 'if ChangesetId::of(changeset) != *id {' 'if false {' --lib
mutate src/state.rs '&& head == *id' '&& false' --test store
mutate src/state.rs 'if changeset.base != state.head {' 'if false {' --test store
mutate src/state.rs 'if state.location != *at {' 'if false {' --lib
mutate src/state.rs '&& last.changeset == *id' '' --test store
mutate src/state.rs '&& last.fork == *from' '' --test store
mutate src/state.rs 'if state.head_changeset.map(|(head, _)| head) != Some(*id) {
                return Err(CoreError::NotHead {
                    fork: from.to_string(),' 'if false {
                return Err(CoreError::NotHead {
                    fork: from.to_string(),' --test store
mutate src/state.rs 'if changeset.base != loc.main {' 'if false {' --test store
mutate src/state.rs 'state.landing_intent
                && last == *id' 'state.landing_intent
                && false' --test store
mutate src/state.rs 'if state.head_changeset.map(|(head, _)| head) != Some(*id) {
                return Err(CoreError::NotHead {
                    fork: name.to_string(),' 'if false {
                return Err(CoreError::NotHead {
                    fork: name.to_string(),' --test store
# The changeset index keeps the first draft that recorded a changeset.
mutate src/store.rs 'if self.changesets.get(id.as_bytes().as_slice())?.is_none() {' 'if true {' --test store
# Replay: offsets, record bodies, duplicates, and each projection table.
mutate src/store.rs 'if record.offset != records {' 'if false {' --lib
mutate src/store.rs 'if record.offset != offset {' 'if false {' --lib
mutate src/store.rs 'return Err(corrupt(format!("duplicates the record at {at}")));' 'let _ = at;' --lib
mutate src/store.rs 'if locations != replayed {' 'if false {' --lib
mutate src/store.rs 'if forks != replayed {' 'if false {' --lib
mutate src/store.rs 'let locations: BTreeMap<String, Location> = txn
            .open_table(LOCATIONS)?
            .iter()?
            .map(|r| {
                let (k, v) = r?;
                Ok((k.value().to_string(), from_json(v.value())?))' 'let locations: BTreeMap<String, Location> = txn
            .open_table(LOCATIONS)?
            .iter()?
            .map(|r| {
                let (_, v) = r?;
                let l: Location = from_json(v.value())?;
                Ok((l.name.to_string(), l))' --lib
mutate src/store.rs 'if index != replayed {' 'if false {' --lib
mutate src/store.rs '.map_or(0, |last| last + 1)' '.map_or(0, |last| last + 2)' --test store
# Trees are rebuilt and checked against their id.
mutate src/store.rs 'if tree.git_tree_id() != root {' 'if false {' --lib
# Names and remotes.
mutate src/names.rs '&& !s.contains("..")' '' --lib
mutate src/names.rs '&& b[0].is_ascii_alphanumeric()' '' --lib
mutate src/store.rs "&& !remote.starts_with('-')" '' --test store
mutate src/store.rs '&& remote.len() <= 512' '' --test store
# Ingest: concurrent ingests into one new shard; leftovers removed; short index rows.
mutate src/blobs.rs 'Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}' '' --test store
mutate src/blobs.rs 'fs::remove_file(path)?;' '' --test store
mutate src/store.rs 'let (Some(sha), Some(size)) = (bytes.get(..32), bytes.get(32..40)) else {
        return Err(CoreError::CorruptBlob(oid));
    };' 'let (sha, size) = (Some(&bytes[..32]).unwrap(), Some(&bytes[32..40]).unwrap());' --lib
# Not covered, by design: the fsyncs of blob files and directories. A SIGKILL keeps the
# page cache, so no test here can tell a missing fsync; power-loss durability of blobs
# rests on reading the code. The redb commit's durability is covered (below).

# Commits are durable before they return: without it, a kill loses acknowledged drafts.
mutate src/store.rs 'txn.set_durability(Durability::Immediate);' 'txn.set_durability(Durability::None);' --test crash

exit $survived
