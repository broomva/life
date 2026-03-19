#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

if [ -n "${CONTROL_WEB_E2E_CMD:-}" ]; then
  eval "$CONTROL_WEB_E2E_CMD"
  exit 0
fi

base_url="${APP_BASE_URL:-${PLAYWRIGHT_BASE_URL:-}}"

if [ -f package.json ] && command -v npm >/dev/null 2>&1; then
  if node -e 'const p=require("./package.json"); process.exit(p.scripts&&p.scripts["e2e:web"]?0:1)' >/dev/null 2>&1; then
    npm run -s e2e:web
    exit 0
  fi
fi

if [ -f playwright.config.ts ] && command -v npx >/dev/null 2>&1; then
  npx playwright test tests/e2e/web --reporter=line
  exit 0
fi

if [ -n "$base_url" ] && command -v curl >/dev/null 2>&1; then
  curl -fsS "$base_url" >/dev/null
  echo "Web deployment reachable: $base_url"
  exit 0
fi

echo "No web e2e command configured. Set CONTROL_WEB_E2E_CMD or APP_BASE_URL, or install Playwright config/tests." >&2
exit 1
