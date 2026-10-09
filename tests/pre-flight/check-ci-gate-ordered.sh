#!/usr/bin/env bash
# ============================================================================
# P4 红相前置断言：CI 分批串行滚动路径（ci.yml / scripts/serialized-rollout.sh）
# 必须存在批次间"drain 收尾等待 → 长流连续性门禁调用"，且顺序正确
# （drain 收尾等待在前，长流门禁在后）。
#
# 判据（机器可判定）：
#   drain 收尾等待模式：  kubectl ... rollout status（批次收敛） 或
#                         Terminating（旧 Pod 缩容收尾等待）
#   长流门禁调用模式：    PROBE_LONG_STREAM | long_stream | long-stream |
#                         ponyllm_synthetic_long_stream | --long-stream
#   顺序断言：            同一文件内 drain 行号 < 长流门禁行号；
#                         任一候选文件完整满足即 PASS。
#
# 红相基线（当前 HEAD）：ci.yml 仅有 apply 后逐 Deployment rollout status
# （无批次间 drain 收尾等待语义组合），无任何长流门禁调用；serialized-rollout.sh
# 不存在 → 本脚本必须失败（exit!=0）。
# 实现转绿：契约 C1 —— serialized-rollout.sh 逐 role 批次内 ① rollout status
# 收敛 ② 旧 ReplicaSet 缩容至 0 / 无 Terminating Pod 等待 ③ 长流连续性门禁。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CANDIDATES=("$REPO_ROOT/.github/workflows/ci.yml")
if [ -f "$REPO_ROOT/scripts/serialized-rollout.sh" ]; then
  CANDIDATES+=("$REPO_ROOT/scripts/serialized-rollout.sh")
fi

DRAIN_RE='kubectl[[:space:]]+[^|;]*rollout[[:space:]]+status|Terminating'
GATE_RE='PROBE_LONG_STREAM|long_stream|long[._-]?stream|ponyllm_synthetic_long_stream|--long-stream'

overall_fail=1
for f in "${CANDIDATES[@]}"; do
  [ -f "$f" ] || continue
  drain_line="$(grep -nEm1 "$DRAIN_RE" "$f" | cut -d: -f1 || true)"
  gate_line="$(grep -nEm1 "$GATE_RE" "$f" | cut -d: -f1 || true)"
  if [ -n "$drain_line" ] && [ -n "$gate_line" ] && [ "$drain_line" -lt "$gate_line" ]; then
    echo "PASS ($f): drain finish-wait at line $drain_line precedes long-stream gate at line $gate_line"
    overall_fail=0
  else
    echo "INFO ($f): drain_line=${drain_line:-MISSING} gate_line=${gate_line:-MISSING}"
  fi
done

if [ "$overall_fail" -eq 0 ]; then
  echo "GREEN: ordered 'drain finish-wait -> long-stream gate' sequence present in CI rollout path (C1/C3 gate wired)"
  exit 0
fi
echo "RED: no ordered 'drain finish-wait -> long-stream gate' sequence found in CI rollout path (C1/C3 gate missing)"
exit 1
