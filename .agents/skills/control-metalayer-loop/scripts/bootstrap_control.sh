#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: bootstrap_control.sh [repo_path] [--force]

Install baseline control metalayer templates into a target repository.
USAGE
}

repo_path="."
force=0

while [ $# -gt 0 ]; do
  case "$1" in
    --force)
      force=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      if [ "$repo_path" != "." ]; then
        echo "error: multiple repo paths provided" >&2
        exit 1
      fi
      repo_path="$1"
      ;;
  esac
  shift
done

if [ ! -d "$repo_path" ]; then
  echo "error: repo path not found: $repo_path" >&2
  exit 1
fi

repo_path=$(cd "$repo_path" && pwd)
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
skill_dir=$(cd "$script_dir/.." && pwd)
template_dir="$skill_dir/assets/templates"

copy_template() {
  local rel="$1"
  local src="$template_dir/$rel"
  local dst="$repo_path/$rel"

  if [ ! -f "$src" ]; then
    echo "[error] missing template: $rel" >&2
    exit 1
  fi

  mkdir -p "$(dirname "$dst")"

  if [ -f "$dst" ] && [ "$force" -ne 1 ]; then
    echo "[skip]  $rel"
    return
  fi

  cp "$src" "$dst"
  echo "[write] $rel"
}

baseline=(
  "AGENTS.md"
  "PLANS.md"
  "METALAYER.md"
  "Makefile.control"
  "scripts/audit_control.sh"
  "scripts/control/smoke.sh"
  "scripts/control/check.sh"
  "scripts/control/test.sh"
  "docs/control/ARCHITECTURE.md"
  "docs/control/OBSERVABILITY.md"
  ".github/workflows/control-harness.yml"
)

for rel in "${baseline[@]}"; do
  copy_template "$rel"
done

makefile="$repo_path/Makefile"
if [ ! -f "$makefile" ]; then
  cat > "$makefile" <<'MAKEFILE'
-include Makefile.control
MAKEFILE
  echo "[write] Makefile"
elif ! grep -Eq '(^|[[:space:]])-?include[[:space:]]+Makefile\.control([[:space:]]|$)' "$makefile"; then
  cat >> "$makefile" <<'MAKEFILE'

# Control metalayer targets
-include Makefile.control
MAKEFILE
  echo "[update] Makefile"
else
  echo "[skip]  Makefile already includes Makefile.control"
fi

chmod +x \
  "$repo_path/scripts/audit_control.sh" \
  "$repo_path/scripts/control/smoke.sh" \
  "$repo_path/scripts/control/check.sh" \
  "$repo_path/scripts/control/test.sh"

echo
echo "Baseline control metalayer bootstrap complete."
echo "Next: run python3 scripts/control_wizard.py audit $repo_path"
