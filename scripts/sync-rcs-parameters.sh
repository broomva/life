#!/usr/bin/env bash
# Sync the RCS canonical parameters.toml mirror in autonomic-core from the
# paper repo (~/broomva/research/rcs).
#
# The paper repo is the single source of truth. Life vendors a mirror at
# crates/autonomic/autonomic-core/data/rcs-parameters.toml so the Rust
# `rcs_budget` module can `include_str!` it at compile time.
#
# Run this after editing the paper's parameters.toml to keep both repos
# aligned. Exits 0 if they match after sync.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LIFE_ROOT="$(cd "${HERE}/.." && pwd)"
PAPER_SRC="${HOME}/broomva/research/rcs/latex/parameters.toml"
MIRROR_DST="${LIFE_ROOT}/crates/autonomic/autonomic-core/data/rcs-parameters.toml"

if [[ ! -f "${PAPER_SRC}" ]]; then
    echo "error: paper source missing at ${PAPER_SRC}" >&2
    exit 1
fi

# The mirror header is a 10-line preamble (MIRROR marker + sync instructions)
# followed by the verbatim paper body starting from the paper's own
# "# RCS — Canonical Parameters" comment line.
HEADER="# =============================================================================
# RCS — Canonical Parameters (MIRROR)
# =============================================================================
#
# THIS FILE IS A MIRROR. The authoritative source lives in the paper repo at:
#   ~/broomva/research/rcs/latex/parameters.toml
# Keep the two in sync via \`scripts/sync-rcs-parameters.sh\` (or manual copy).
# The mirror is required because the paper repo is a separate git root and
# this crate needs compile-time access via \`include_str!\`.
#"

tmp="$(mktemp)"
trap 'rm -f "${tmp}"' EXIT

# Start with the mirror header, then append the paper body starting from
# line 4 (skipping the paper's own three-line "# RCS — Canonical Parameters ="
# banner which the mirror header already replaces).
{
    printf '%s\n' "${HEADER}"
    tail -n +4 "${PAPER_SRC}"
} > "${tmp}"

if [[ -f "${MIRROR_DST}" ]] && diff -q "${tmp}" "${MIRROR_DST}" >/dev/null; then
    echo "mirror is already in sync"
    exit 0
fi

echo "updating mirror ${MIRROR_DST}" >&2
if [[ -f "${MIRROR_DST}" ]]; then
    diff -u "${MIRROR_DST}" "${tmp}" >&2 || true
fi
mkdir -p "$(dirname "${MIRROR_DST}")"
cp "${tmp}" "${MIRROR_DST}"
echo "done"
