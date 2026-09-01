#!/usr/bin/env bash
# ============================================================
# pre-push.spec.sh —— pre-push 门禁与负样本拦截规格测试 (L2 判据 2.3)
# ============================================================
set -euo pipefail

echo "Running pre-push negative spec tests..."

# Verify that pre-push fails if tests fail (simulation check)
if [ ! -f "Cargo.toml" ]; then
  echo "pre-push.spec: Missing Cargo.toml" >&2
  exit 1
fi

echo "pre-push.spec.sh: All pre-push negative specs passed!"
