#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

echo "Recovery workflow"
echo "1) Re-run smoke"
echo "2) Re-run check"
echo "3) Capture failing tests and open escalation if needed"

./scripts/control/smoke.sh || true
./scripts/control/check.sh || true
./scripts/control/test.sh || true
