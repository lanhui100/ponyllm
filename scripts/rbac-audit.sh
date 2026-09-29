#!/usr/bin/env bash
# RBAC audit for ponyllm-gateway-sa (sec S3, T13) — READ-ONLY.
#
# Runs the full kubectl auth can-i matrix from the Phase 2/3 security review:
# the gateway SA must be able to get+patch EXACTLY ONE secret
# (ponyllm-live-config) and nothing else — no list/watch/create/delete/update,
# no other namespaces, no other resource types. Non-zero exit on any mismatch.
#
# Usage:
#   export NS=ponyllm
#   bash scripts/rbac-audit.sh
set -euo pipefail

NS="${NS:-ponyllm}"
SA="system:serviceaccount:$NS:ponyllm-gateway-sa"
KUBECTL=(kubectl)
[ -n "${KUBECTL_CTX:-}" ] && KUBECTL=(kubectl --context "$KUBECTL_CTX")

# case format: expected|verb|resource|namespace(empty = $NS)
CASES=(
  # --- 白名单内的唯二放行 ---
  "yes|get|secrets/ponyllm-live-config|"
  "yes|patch|secrets/ponyllm-live-config|"
  # --- 同 ns 其他 Secret 一律拒绝 ---
  "no|get|secrets/ponyllm-config|"
  "no|get|secrets/aliyun-registry|"
  "no|get|secrets/ponyllm-lock-dsn|"
  "no|get|secrets/ponyllm-lock-tls|"
  "no|get|secrets/ponyllm-telemetry-snapshot|"
  # --- 未指名/批量操作一律拒绝（resource 用无斜杠的 "secrets" 表示整类） ---
  "no|get|secrets|"
  "no|list|secrets|"
  "no|watch|secrets|"
  # --- 写面仅 patch，禁 create/update/delete（patch-CAS 决策） ---
  "no|create|secrets/ponyllm-live-config|"
  "no|update|secrets/ponyllm-live-config|"
  "no|delete|secrets/ponyllm-live-config|"
  # --- 其他资源类型 ---
  "no|get|configmaps|"
  "no|get|pods|"
  "no|get|deployments|"
  # --- 跨命名空间（kube-system / production 均为真实 ns；SA 不得触碰） ---
  "no|get|secrets|kube-system"
  "no|get|secrets|production"
)

FAIL=0
for c in "${CASES[@]}"; do
  IFS='|' read -r EXPECTED VERB RES NSP <<< "$c"
  if [ -n "$NSP" ]; then
    OUT=$( "${KUBECTL[@]}" -n "$NSP" auth can-i "$VERB" "$RES" --as="$SA" 2>/dev/null || true )
  else
    OUT=$( "${KUBECTL[@]}" -n "$NS" auth can-i "$VERB" "$RES" --as="$SA" 2>/dev/null || true )
  fi
  if [ "$OUT" = "$EXPECTED" ]; then
    echo "PASS $VERB $RES (ns=${NSP:-$NS}) = $OUT"
  else
    echo "FAIL $VERB $RES (ns=${NSP:-$NS}) expected $EXPECTED, got '$OUT'"
    FAIL=1
  fi
done

if [ "$FAIL" = "0" ]; then
  echo "== rbac-audit PASS: ponyllm-gateway-sa 权限面与最小权限矩阵完全一致 =="
else
  echo "== rbac-audit FAIL: 存在越权/缺权项，立即排查（RBAC 变更应先经安全评审） =="
  exit 1
fi