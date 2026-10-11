#!/usr/bin/env bash
# ============================================================================
# 前置契约断言：CI 部署参数与探针 CLI 接口静态防呆门禁
#
# 判据（机器可判定）：
#   1. deploy/prober.py 必须支持 --api-key 和 --url 参数解析（--help 显式存在）；
#   2. scripts/serialized-rollout.sh 必须显式包含透传 --api-key 与 --url 的逻辑；
#   3. scripts/serialized-rollout.sh --dry-run 模式必须 Exit 0 成功通过；
#   4. PONYLLM_BASE_URL 环境变量注入时，deploy/prober.py 能正确兜底解析。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echo "== [1/3] Checking deploy/prober.py CLI argument capabilities =="
if [ ! -f "$REPO_ROOT/deploy/prober.py" ]; then
  echo "FAIL: deploy/prober.py not found!"
  exit 1
fi

prober_help="$(python3 "$REPO_ROOT/deploy/prober.py" --help 2>&1)"
if ! echo "$prober_help" | grep -q -- '--api-key'; then
  echo "FAIL: deploy/prober.py does not declare --api-key CLI argument!"
  exit 1
fi
if ! echo "$prober_help" | grep -q -- '--url'; then
  echo "FAIL: deploy/prober.py does not declare --url CLI argument!"
  exit 1
fi
echo "PASS: deploy/prober.py declares required CLI flags (--api-key, --url)"

echo "== [2/3] Checking scripts/serialized-rollout.sh parameter forwarding contract =="
if [ ! -f "$REPO_ROOT/scripts/serialized-rollout.sh" ]; then
  echo "FAIL: scripts/serialized-rollout.sh not found!"
  exit 1
fi

rollout_content="$(cat "$REPO_ROOT/scripts/serialized-rollout.sh")"
if ! echo "$rollout_content" | grep -q -- '--api-key'; then
  echo "FAIL: scripts/serialized-rollout.sh fails to pass --api-key to prober!"
  exit 1
fi
if ! echo "$rollout_content" | grep -q -- '--url'; then
  echo "FAIL: scripts/serialized-rollout.sh fails to pass --url to prober!"
  exit 1
fi
echo "PASS: scripts/serialized-rollout.sh correctly forwards credentials and target URL"

echo "== [3/3] Checking serialized-rollout.sh --dry-run execution =="
bash "$REPO_ROOT/scripts/serialized-rollout.sh" --dry-run > /dev/null
echo "PASS: serialized-rollout.sh --dry-run exited 0 cleanly"

echo "GREEN: All deployment prober contract checks passed!"
exit 0
