#!/usr/bin/env bash
# Mutation check for cuenca-tree: each mutant breaks one guarantee, and the tests must
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
  elif grep -qE '^error\[E[0-9]+\]|could not compile|failed to select a version' "$backup/log"; then
    echo "NO BUILD  $file: $old (a mutant that does not build proves nothing)"; survived=1
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
mutate Cargo.toml 'unicase = "=2.10.0"' 'unicase = "=2.9.0"' --test git_bridge fold_collisions_match_apfs
mutate src/path.rs "        if c == b':' {
            return true;" "        if c == b':' {
            return false;" --lib
mutate src/tree.rs 'if is_gitmodules_alias(name) {' 'if false {' --test git_bridge
mutate src/tree.rs 'if is_gitattributes_alias(name) && (is_dir || entry.mode() == Mode::Symlink) {' 'if false {' --test git_bridge
mutate src/tree.rs '.find(|l| l.len() >= MAX_GITATTRIBUTES_LINE)' '.find(|l| l.len() > 4 * MAX_GITATTRIBUTES_LINE)' --lib
mutate src/changeset.rs 'if actual != self.target {' 'if false {' --lib
mutate src/git.rs '.env("GIT_NO_REPLACE_OBJECTS", "1")' '' --test git_bridge
mutate src/git.rs '.env("GIT_NO_LAZY_FETCH", "1")' '' --test git_bridge
mutate src/git.rs 'Some(blob) if blob.oid() == oid => Ok(()),' 'Some(_) => Ok(()),' --test git_bridge
mutate src/git.rs 'Ok(blob) if blob.oid() == oid => Ok(()),' 'Ok(_) => Ok(()),' --test git_bridge
mutate src/git.rs 'if let Some(empty) = dirs.iter().find(|d| !parents.contains(d.as_str()))' 'if let Some(empty) = dirs.iter().find(|_| false)' --test git_bridge
mutate src/git.rs '"sha1" => {}' '_ => {}' --test git_bridge
mutate src/git.rs 'return Err(GitError::PartialClone);' '' --test git_bridge
mutate src/git.rs '[id, _, _] if *id == oid.to_hex() => return Err(GitError::WrongType(*oid)),' '' --test git_bridge
mutate src/git.rs 'verify_bytes(*oid, content)?;' '' --test git_bridge
mutate src/path.rs '|| folds_to(component, ".gitmodules")' '' --lib
mutate src/git.rs 'self.read_tree(&root.to_hex())?;' '' --test git_bridge
mutate src/git.rs 'Ok(_) => return Err(GitError::Alternates),' 'Ok(_) => {}' --test git_bridge
mutate src/git.rs 'Ok(bytes) if bytes.is_empty() => {}' 'Ok(bytes) if bytes.iter().all(|b| b.is_ascii_whitespace()) => {}' --test git_bridge
mutate src/git.rs 'ruled[&oid].check_stream(oid, size, content)' 'Ok(())' --test git_bridge
mutate src/git.rs 'return Err(GitError::NonCanonical { stored: root, ours });' '' --test git_bridge
mutate src/git.rs 'rules.check(content)?;' '' --test git_bridge
mutate src/git.rs 'None => hash_stream(oid, size, content),' 'None => Ok(()),' --test git_bridge
mutate src/git.rs 'Some(rules) => rules.check_stream(oid, size, content),' 'Some(_) => Ok(()),' --test git_bridge
mutate src/tree.rs '} else if content.len() > MAX_SYMLINK_TARGET {' '} else if false {' --lib
mutate src/git.rs 'if stored != *expected {' 'if false {' --test git_bridge substitutes_the_listing
mutate src/git.rs 'if ours != root {' 'if false {' --test git_bridge reading_a_tree_with_a_corrupt_subtree
mutate src/tree.rs 'Some(SymlinkProblem::Empty)' 'None' --lib
mutate src/tree.rs 'Some(SymlinkProblem::Nul)' 'None' --lib
mutate src/git.rs 'check_content(path, Mode::Symlink, content)?;' '' --test git_bridge
mutate src/oid.rs 'Sha1::builder().safe_hash(false).build()' 'Sha1::builder().safe_hash(false).detect_collision(false).build()' --lib
mutate src/path.rs 'pub const MAX_PATH_BYTES: usize = 1023;' 'pub const MAX_PATH_BYTES: usize = 4096;' --lib
mutate src/oid.rs 'CollisionResult::Mitigated(_) | CollisionResult::Collision(_) => None,' 'CollisionResult::Mitigated(d) | CollisionResult::Collision(d) => Some(d.into()),' --lib

exit $survived
