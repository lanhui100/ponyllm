#!/usr/bin/env bash
# ============================================================================
# P2 红相前置断言：4 个 gateway Deployment（dev/preprod/proserver/tencent）
# 副本数断言（对齐 Lead 裁决契约 C2）：
#   - dev: replicas=1 伴随注释标记 "contract C2: dev single-writer exception"
#          （或 replicas>=2 / 蓝绿双 RS）
#   - preprod/proserver/tencent: replicas>=2（或等价蓝绿双 RS）
# 且 dev PVC 单写者约束显式处置标记必须存在。
#
# 判据（机器可判定）：
#   1. 解析 deploy/ponyllm-deployment.yaml 每个 Deployment 的 name + spec.replicas；
#   2. 对 4 个 role 逐一断言：
#      - dev: 若 replicas == 1 且清单包含 "contract C2: dev single-writer exception"
#        注释标记，判定为 PASS；或 replicas >= 2 / 蓝绿双 RS；
#      - preprod / proserver / tencent: 仍必须 replicas >= 2（或等价蓝绿双 RS）；
#   3. 断言清单内含 dev PVC 单写者显式处置标记。
#
# 红相基线（当前 HEAD）：4 个 Deployment 全部 replicas:1 且清单无 "contract C2:
# dev single-writer exception" 注释标记（当前未添加）且其余 3 个全部 replicas:1
# → 本脚本在当前 HEAD 下依然全红（exit!=0）。
# 实现转绿：executor-b 实施 C2 裁决后转绿。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MANIFEST="$REPO_ROOT/deploy/ponyllm-deployment.yaml"

declare -A replicas
declare -A deployed
while read -r name rep; do
  [ -n "$name" ] || continue
  deployed["$name"]=1
  replicas["$name"]="${rep:-0}"
done < <(awk '
  /^kind: Deployment/ { in_deploy=1; dname=""; drep="" }
  in_deploy && /^  name: / { dname=$2 }
  in_deploy && /^  replicas: / { drep=$2 }
  /^---/ { if (in_deploy && dname!="") print dname, drep; in_deploy=0 }
  END { if (in_deploy && dname!="") print dname, drep }
' "$MANIFEST")

role_ok() { # $1 = role (dev/preprod/proserver/tencent)
  local role="$1"
  local exact="ponyllm-gateway-$role"
  local n="${replicas[$exact]:-0}"

  # Lead 裁决契约 C2 特别处置：dev 保持 replicas=1 伴随 contract C2 注释标记
  if [ "$role" = "dev" ]; then
    if [ "$n" -eq 1 ] && grep -qF "contract C2: dev single-writer exception" "$MANIFEST"; then
      echo "PASS: ponyllm-gateway-dev replicas=1 with explicit 'contract C2: dev single-writer exception' annotation (C2 Lead decision)"
      return 0
    fi
  fi

  if [ "$n" -ge 2 ] 2>/dev/null; then
    echo "PASS: $exact replicas=$n (>=2, C2 satisfied)"
    return 0
  fi
  # 等价蓝绿双 RS：同一 role 存在 blue/green 或 a/b 双 Deployment
  for pair in "blue green" "a b"; do
    set -- $pair
    if [ "${deployed[$exact-$1]:-0}" = 1 ] && [ "${deployed[$exact-$2]:-0}" = 1 ]; then
      echo "PASS: $role blue-green dual RS ($exact-$1 / $exact-$2)"
      return 0
    fi
  done
  if [ "$role" = "dev" ]; then
    echo "FAIL: $exact replicas=$n (dev exception requires replicas=1 AND 'contract C2: dev single-writer exception' comment marker; comment missing) — C2 violated"
  else
    echo "FAIL: $exact replicas=$n (<2, no blue-green dual RS) — C2 replicas floor violated"
  fi
  return 1
}

fail=0
for role in dev preprod proserver tencent; do
  role_ok "$role" || fail=1
done

# dev 的 PVC 单写者约束显式处置：契约标记（telemetry single-writer / 单写者 /
# dev-only PVC 注释等，等价形式均可）
if grep -qiE 'single-?writer|单写者|telemetry.{0,60}(persist|write)|仅[[:space:]]*dev[^[:alnum:]]*.{0,30}(PVC|副本|mount|挂)' "$MANIFEST"; then
  echo "PASS: dev PVC (ponyllm-data RWO) single-writer constraint has explicit disposition marker"
else
  echo "FAIL: no explicit single-writer disposition marker for dev PVC (ponyllm-data RWO) in manifest"
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "GREEN: all 4 gateway Deployments meet C2 replicas floor (>=2 or dual RS) + single-writer disposition explicit"
  exit 0
fi
echo "RED: one or more gateway Deployments below C2 replicas floor / single-writer disposition missing"
exit 1
