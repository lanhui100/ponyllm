#!/usr/bin/env bash
# ============================================================================
# P3 红相前置断言：deploy/prober.py 必须含长流连续性模式（SSE stream=true、
# truncation 判据、ponyllm_synthetic_long_stream_* metrics 行），
# deploy/ponyllm-prober.yaml 必须注入长流 env（PROBE_LONG_STREAM 或等价）。
#
# 判据（机器可判定，grep 清单）：
#   a. 长流 SSE 请求模式：  '"stream": true' 或 'stream=true'（payload/入参）
#   b. truncation 判据：     truncat | finish_reason | stop_reason | clean-truncat
#                            （干净截断 vs 提前中断的判定逻辑，契约 C3）
#   c. 长流 metrics 行：     ponyllm_synthetic_long_stream_ 前缀
#   d. 长流 env 注入：       deploy/ponyllm-prober.yaml 含 PROBE_LONG_STREAM
#                            或等价 long-stream 标记
#
# 红相基线（当前 HEAD）：prober.py 无任何 stream 逻辑、prober yaml 无长流 env
# → 四项全缺 → 本脚本必须失败（exit!=0）。
# 实现转绿：按契约 C3 实现长流模式 + metrics + env 注入（ConfigMap 同步步骤见契约）。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROBER="$REPO_ROOT/deploy/prober.py"
PROBER_YAML="$REPO_ROOT/deploy/ponyllm-prober.yaml"

[ -f "$PROBER" ] || { echo "FAIL: $PROBER missing"; exit 1; }
[ -f "$PROBER_YAML" ] || { echo "FAIL: $PROBER_YAML missing"; exit 1; }

fail=0

# (a) SSE 长流请求模式
if grep -qE '"stream"[[:space:]]*:[[:space:]]*true|stream[[:space:]]*=[[:space:]]*true|stream=[Tt]rue' "$PROBER"; then
  echo "PASS: prober.py contains SSE stream=true long-stream mode (C3.a)"
else
  echo "FAIL: prober.py missing SSE stream=true long-stream mode (C3.a)"
  fail=$((fail + 1))
fi

# (b) truncation 判据
if grep -qiE 'truncat|finish_reason|stop_reason|clean[_-]?truncat' "$PROBER"; then
  echo "PASS: prober.py contains truncation criterion (C3.b)"
else
  echo "FAIL: prober.py missing truncation criterion (C3.b)"
  fail=$((fail + 1))
fi

# (c) 长流 metrics 行
if grep -qE 'ponyllm_synthetic_long_stream_' "$PROBER"; then
  echo "PASS: prober.py exposes ponyllm_synthetic_long_stream_* metrics (C3.c)"
else
  echo "FAIL: prober.py missing ponyllm_synthetic_long_stream_* metrics lines (C3.c)"
  fail=$((fail + 1))
fi

# (d) prober Deployment 注入长流 env
if grep -qE 'PROBE_LONG_STREAM|LONG_STREAM|long[_-]?stream' "$PROBER_YAML"; then
  echo "PASS: ponyllm-prober.yaml injects long-stream env (C3.d)"
else
  echo "FAIL: ponyllm-prober.yaml missing long-stream env PROBE_LONG_STREAM or equivalent (C3.d)"
  fail=$((fail + 1))
fi

if [ "$fail" -eq 0 ]; then
  echo "GREEN: prober long-stream continuity mode fully present (C3 satisfied)"
  exit 0
fi
echo "RED: prober long-stream continuity mode missing ($fail/4 assertions failed, C3 violated)"
exit 1
