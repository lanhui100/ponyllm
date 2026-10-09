#!/usr/bin/env bash
# ============================================================================
# P1 红相前置断言：CI 部署路径不得存在"单次 kubectl apply 同时含全部 4 个
# Deployment"模式。
#
# 判据（机器可判定）：
#   1. 折叠反斜杠续行为逻辑语句（ci.yml / scripts/serialized-rollout.sh）；
#   2. 任一逻辑语句若同时含 `kubectl apply` 与 `deploy/ponyllm-deployment.yaml`
#      （该清单内含 4 个 gateway Deployment: dev/preprod/proserver/tencent），
#      则该次 apply 覆盖全部 4 个 Deployment —— 除非语句携带 role 限定
#      selector（ponyllm.io/node-role，如 `-l ponyllm.io/node-role=dev` 或
#      `--selector=ponyllm.io/node-role=dev`，契约 C1 推荐机制）；
#   3. 存在上述无 role 限定的全量 apply → 断言失败（红相成立，非零退出）。
#
# 红相基线（当前 HEAD）：ci.yml "Apply manifests" 步骤单次 apply 两个清单
# （deployment.yaml 含全部 4 个 Deployment、无 role selector）→ 本脚本必须失败。
# 实现转绿：逐 role 串行 apply（-l ponyllm.io/node-role=<role>）或按 role 拆清单。
# 非 Deployment 资源（IngressRoute 等）无滚动语义，可单次 apply，不在此断言内。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CANDIDATES=("$REPO_ROOT/.github/workflows/ci.yml")
if [ -f "$REPO_ROOT/scripts/serialized-rollout.sh" ]; then
  CANDIDATES+=("$REPO_ROOT/scripts/serialized-rollout.sh")
fi

# 把反斜杠续行的物理行折叠为逻辑语句
fold_stmts() { # $1 = file
  awk '
    {
      if (prev) buf = buf " " $0; else buf = $0
      if ($0 ~ /\\[ \t]*$/) { prev = 1 } else { prev = 0; print buf; buf = "" }
    }
    END { if (prev && buf != "") print buf }
  ' "$1"
}

violations=0
for f in "${CANDIDATES[@]}"; do
  [ -f "$f" ] || continue
  while IFS= read -r stmt; do
    [[ "$stmt" =~ kubectl[[:space:]]+apply ]] || continue
    [[ "$stmt" =~ ponyllm-deployment\.yaml ]] || continue
    # role 限定 selector 存在 → 该次 apply 仅覆盖单 role Deployment，允许
    if [[ "$stmt" =~ ponyllm\.io/node-role ]]; then
      echo "PASS: role-scoped apply (single role, not all 4): $stmt"
      continue
    fi
    echo "FAIL: single kubectl apply covers all 4 Deployments (no role selector): $stmt"
    violations=$((violations + 1))
  done < <(fold_stmts "$f")
done

if [ "$violations" -gt 0 ]; then
  echo "RED: $violations anti-pattern apply statement(s) found in CI apply paths (C1 serialized rollout violated)"
  exit 1
fi
echo "GREEN: no single kubectl apply covers all 4 Deployments in CI apply paths (C1 satisfied)"
exit 0
