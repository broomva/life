#!/usr/bin/env bash
# arcan-skills-add — wrapper around `npx skills add` that auto-syncs to .arcan/skills/
#
# Usage: ./scripts/arcan-skills-add.sh <source> [options]
# Example: ./scripts/arcan-skills-add.sh vercel-labs/agent-skills -g
#
# After installing via `npx skills add`, runs `arcan skills sync` to create
# symlinks in .arcan/skills/ so the skill is immediately available on next
# `arcan serve` without waiting for a restart.

set -euo pipefail

if [ $# -eq 0 ]; then
    echo "Usage: arcan-skills-add <source> [npx skills options]"
    echo "Example: arcan-skills-add vercel-labs/agent-skills -g"
    exit 1
fi

# Step 1: Install the skill via npx skills
echo "Installing skill via npx skills add $*..."
npx skills add "$@"

# Step 2: Sync into .arcan/skills/
ARCAN_BIN=$(command -v arcan 2>/dev/null || echo "")
if [ -n "$ARCAN_BIN" ]; then
    echo ""
    echo "Syncing skills into .arcan/skills/..."
    arcan skills sync
elif [ -f ".arcan/skills" ] || [ -d ".arcan" ]; then
    echo ""
    echo "Syncing skills into .arcan/skills/..."
    cargo run -p arcan -- skills sync 2>/dev/null || echo "(arcan not built, skipping sync — skills will auto-sync on next arcan serve)"
else
    echo ""
    echo "Note: Run 'arcan skills sync' to create .arcan/skills/ symlinks, or skills will auto-sync on next 'arcan serve'."
fi
