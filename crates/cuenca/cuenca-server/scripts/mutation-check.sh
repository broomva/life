#!/usr/bin/env bash
# Mutation check for cuenca-server: each mutant breaks one guarantee, and the tests must
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
if ! cargo test -q -p cuenca-server >"$backup/log" 2>&1; then
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
  if cargo test -q -p cuenca-server "$@" >"$backup/log" 2>&1; then
    echo "SURVIVED  $file: $old"; survived=1
  elif grep -qE '^error\[E[0-9]+\]|could not compile|failed to select a version' "$backup/log"; then
    echo "NO BUILD  $file: $old (a mutant that does not build proves nothing)"; survived=1
  else
    echo "killed    $file: $old"
  fi
  restore
}

# Keys: only the right secret, with the right prefix, verifies; one key per principal;
# removal ends a key; the file is private.
mutate src/keys.rs 'constant_time_eq(presented.as_bytes(), record.secret_sha256.as_bytes())' 'true' --lib
mutate src/keys.rs 'if prefix != TOKEN_PREFIX {' 'if false {' --lib
mutate src/keys.rs 'if keys.values().any(|k| k.principal == principal) {' 'if false {' --lib
mutate src/keys.rs 'next.retain(|_, k| &k.principal != principal);' '' --lib
mutate src/keys.rs 'if mode & 0o077 != 0 {' 'if false {' --lib
mutate src/keys.rs 'file.set_permissions(std::fs::Permissions::from_mode(0o600))?;' 'file.set_permissions(std::fs::Permissions::from_mode(0o644))?;' --lib
# Authentication on every request: revocation in the log, the key's node, routes, an
# unnamed source.
mutate src/authority.rs '            .is_some()
        {
            return Err(AuthError::Revoked' '            .is_some()
            && false
        {
            return Err(AuthError::Revoked' --test api
mutate src/authority.rs 'if peer.node != key.node {' 'if false {' --test api
mutate src/authority.rs 'if peer.serves_routes {' 'if false {' --test api
mutate src/authority.rs '.ok_or(AuthError::UnnamedSource(ip))?;' '.unwrap_or(crate::identity::Peer { node: key.node.clone(), serves_routes: false });' --test api
# Read scoping.
mutate src/authority.rs 'self.owner_device() || self.key.grant.locations.contains(&location.name)' 'true' --test api
mutate src/authority.rs '            || self.key.grant.forks.contains(&fork.name)
' '' --test api
mutate src/authority.rs '.any(|h| h.principal == self.key.principal)' '.any(|_| true)' --test api
mutate src/authority.rs '.any(|h| h.principal == self.key.principal)' '.any(|_| false)' --test api
mutate src/authority.rs 'self.key.kind == PrincipalKind::OwnerDevice' 'true' --test api
mutate src/http.rs 'Some(f) if caller.sees_fork(&f) => Ok(f),' 'Some(f) => Ok(f),' --test api
mutate src/http.rs 'Some(l) if caller.sees_location(&l) => Ok(l),' 'Some(l) => Ok(l),' --test api
mutate src/http.rs '        visible_location(a, &c, &location)?;
' '' --test api
mutate src/http.rs '        visible_fork(a, &c, &name)?;
        to_json(a.store.fork_history' '        to_json(a.store.fork_history' --test api
mutate src/http.rs '        visible_fork(a, &c, &name)?;
        let writer = Writer::new(c.holder.clone(), req.epoch);
        let p =' '        let writer = Writer::new(c.holder.clone(), req.epoch);
        let p =' --test api
mutate src/http.rs '        visible_fork(a, &c, &name)?;
        let writer = Writer::new(c.holder.clone(), req.epoch);
        let offset' '        let writer = Writer::new(c.holder.clone(), req.epoch);
        let offset' --test api
mutate src/http.rs 'if !a.blob_set(tree)?.contains(&oid) {' 'if false {' --test api
# Blob upload: verified even when the store already holds the id.
mutate src/http.rs 'if blob.oid() != oid {' 'if false {' --test api
# Binding: tailnet ranges only, unless unsafe.
mutate src/lib.rs 'if unsafe_bind || is_tailnet(addr.ip()) {' 'if true {' --lib
mutate src/lib.rs 'o[0] == 100 && (o[1] & 0xc0) == 64' 'o[0] == 100' --lib
mutate src/lib.rs 's[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0' 's[0] == 0xfd7a' --lib
# Owner acts: revocation goes to the log, a revoked name is not minted again, the
# socket is private.
mutate src/admin.rs 'let offset = a.store.revoke(&principal).map_err(|e| s(&e))?;' 'let offset = 0;' --test api
mutate src/admin.rs 'if a.store.revoked(&principal).map_err(|e| s(&e))?.is_some() {' 'if false {' --test api
mutate src/admin.rs 'std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;' '' --test api
# Identity: routes from either source mark a peer; a node outside the netmap is unnamed.
mutate src/identity.rs 'serves_routes: routes(&whois.node.primary_routes) || routes(&entry.primary_routes),' 'serves_routes: routes(&entry.primary_routes),' --lib
mutate src/identity.rs 'serves_routes: routes(&whois.node.primary_routes) || routes(&entry.primary_routes),' 'serves_routes: routes(&whois.node.primary_routes),' --lib
mutate src/identity.rs '    let Some(entry) = in_netmap else {
        return Ok(None);
    };' '    let Some(entry) = in_netmap else {
        return Ok(Some(Peer { node, serves_routes: false }));
    };' --lib

exit $survived
