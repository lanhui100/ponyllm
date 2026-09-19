#!/usr/bin/env bash
# ============================================================
# pre-commit.spec.sh —— pre-commit 门禁与负样本拦截规格测试 (L2 判据 2.3)
# ============================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "Running pre-commit gate negative spec tests..."

# Run negative regression spec against verify-note
bash "$ROOT_DIR/.agents/skills/write-adr/tests/test-verify-note.sh"

echo "pre-commit.spec.sh: All negative spec tests passed!"
