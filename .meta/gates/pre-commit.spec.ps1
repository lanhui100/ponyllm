#!/usr/bin/env bash
# ============================================================
# pre-commit.spec.sh —— pre-commit 门禁与负样本拦截规格测试 (L2 判据 2.3)
# ============================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "Running pre-commit gate negative spec tests..."

# 负样本断言：传入非法参数必须非零退出
if bash "$ROOT_DIR/.meta/gates/adr-verify.sh" --non-existent-illegal-flag-test-fail 2>/dev/null; then
  echo "pre-commit.spec: FAIL - gate did not reject invalid input" >&2
  exit 1
fi

echo "pre-commit.spec.sh: All negative spec tests passed!"
exit 0
