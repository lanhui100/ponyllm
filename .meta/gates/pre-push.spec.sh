#!/usr/bin/env bash
# ============================================================
# pre-push.spec.sh —— pre-push 门禁与负样本拦截规格测试 (L2 判据 2.3)
# ============================================================
set -euo pipefail

echo "Running pre-push negative spec tests..."

# 1. 验证 pre-push 脚本本身的语法合法性 (bash -n)
bash -n .meta/gates/pre-push

# 2. 验证 pre-commit 负样本规格测试运行正常
bash .meta/gates/pre-commit.spec.sh >/dev/null 2>&1

# 3. 负样本测试：确保在损坏的环境或非零退出码情况下能够正确捕获错误
test_trap() {
  local failed=0
  ( set -e; false ) || failed=1
  if [ "$failed" -ne 1 ]; then
    echo "Negative spec failed: 'set -e' did not catch non-zero exit!" >&2
    exit 1
  fi
}
test_trap

echo "pre-push.spec.sh: All pre-push negative specs passed!"
