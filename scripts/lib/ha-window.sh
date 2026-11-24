#!/usr/bin/env bash
# ============================================================================
# ha-window.sh —— HA 计数器的「同副本窗口」采样与判定（发布验收脚本共用）
#
# 为什么存在（实证 2026-11-24，run 45418316435）：
#   HA 计数器（refresh_lock_*_total / config_reload_total）是**进程内** AtomicU64，
#   各副本独立累加。经 LB / 公网基址做窗口采样时，前后两次快照可能落到不同副本，
#   于是「换了副本」与「计数器真增长」在数值上无法区分：
#     [5/7] 30s 窗口: acquired 0->32539 | skipped 0->18110 | errors 0->191
#     FAIL refresh_lock_error_total delta +191 != 0
#   实测同一时刻逐副本真值：dev pod err=191（自 48 分钟前启动即恒定，三次采样不变），
#   其余 6 个副本全 0 —— 191 是 dev 副本的历史累计，不是本窗口新增。
#   旧口径只看「正 delta」就判 FAIL，把副本切换误判成锁错误增长。
#
# 契约（机器可判定）：
#   ha_snapshot_line <url> [max_time] [attempts] [auth_token]
#     输出一行 `"<instance_id> <acquired> <skipped> <error>"`。
#     不可达 / 非 JSON / 缺 ha_ops → 输出空行（调用方据此判「证据不足」）。
#   ha_window_verdict <before_file> <after_file>
#     两个文件各含若干 ha_snapshot_line 输出（可空行）。
#     按 instance_id 分组，**只在同副本**的前后样本之间判 delta：
#       任一同副本样本组 error delta > 0        → FAIL（exit 1）
#       全部同副本样本组 error delta == 0 且有活动 → OK（exit 0）
#       同副本样本组存在但无刷新活动           → WARN 证据不足（exit 0）
#       没有任何 instance 同时有前后样本        → WARN 跨副本/身份不可得（exit 0）
#     最后一档是本次修复的要害：跨副本采样不构成「锁错误增长」的证据，
#     不得据以 FAIL；需要硬证据时用 `phase3-verify.sh --pod-ips`（逐副本直采）。
#
# 依赖：python3。无副作用、不写盘。
# ============================================================================

# shellcheck shell=bash

# 单次快照：解析 /v1/telemetry/metrics 的 ha_ops + 副本身份。
ha_snapshot_line() {
  local url="$1" max_time="${2:-8}" attempts="${3:-3}" token="${4:-}"
  local body
  body=$(http_body_retry "$url" "$max_time" "$attempts" "$token" 2>/dev/null || true)
  [ -n "$body" ] || {
    printf '\n'
    return 0
  }
  printf '%s' "$body" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
    ha = d["ha_ops"]
    print(
        d.get("instance_id") or "-",
        ha["refresh_lock_acquired_total"],
        ha["refresh_lock_skipped_total"],
        ha["refresh_lock_error_total"],
    )
except Exception:
    print()
' 2>/dev/null || printf '\n'
}

# 窗口判定：两个快照文件（每行一条 ha_snapshot_line 输出）。
# stdout = 判定文本（可多行）；exit 0 放行 / 1 拒绝。
#
# 输出**只用 ASCII**（实证 2026-11-24 CI windows-latest）：GitHub runner 的
# python3 默认 stdout 编码是 cp1252，print 中文直接 UnicodeEncodeError，
# 整个判定函数 traceback 退出 1——门禁脚本不得依赖终端编码。
ha_window_verdict() {
  local before="$1" after="$2"
  python3 - "$before" "$after" <<'PY'
import sys
from collections import OrderedDict

def load(path):
    """Read snapshot file -> [(instance, acq, skip, err)]; skip blank/malformed."""
    rows = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            parts = line.split()
            if len(parts) != 4:
                continue
            inst, acq, skip, err = parts
            try:
                rows.append((inst, int(acq), int(skip), int(err)))
            except ValueError:
                continue
    return rows

before = load(sys.argv[1])
after = load(sys.argv[2])

if not before or not after:
    print("WARN empty snapshot (before=%d after=%d rows) -- cannot judge lock error delta, insufficient evidence"
          % (len(before), len(after)))
    sys.exit(0)

# Per instance keep the last row on each side: counters are monotonic within one
# process, so the endpoints cover the whole window.
b_last = OrderedDict((r[0], r) for r in before)
a_last = OrderedDict((r[0], r) for r in after)
paired = [i for i in b_last if i in a_last]

if not paired:
    print("WARN no replica sampled on both sides (before=%s after=%s) -- sampling through a LB hits "
          "random replicas; a cross-replica delta is NOT evidence of lock error growth. "
          "For hard evidence use --pod-ips to sample each replica directly"
          % (",".join(b_last), ",".join(a_last)))
    sys.exit(0)

failed = False
activity = False
for inst in paired:
    _, a0, s0, e0 = b_last[inst]
    _, a1, s1, e1 = a_last[inst]
    delta_err = e1 - e0
    print("  replica %s: acquired %d->%d | skipped %d->%d | errors %d->%d"
          % (inst, a0, a1, s0, s1, e0, e1))
    if delta_err > 0:
        print("FAIL replica %s refresh_lock_error_total delta +%d != 0" % (inst, delta_err))
        failed = True
    if (a1 - a0) != 0 or (s1 - s0) != 0:
        activity = True

if failed:
    sys.exit(1)

if not activity:
    print("WARN no refresh activity within the same replica -- delta==0 is insufficient evidence, "
          "re-check on the next keepalive/401 window")
else:
    print("OK same-replica window: lock errors delta==0 with refresh activity")

# Replicas seen on one side only mean a replica switch happened inside the window:
# report it honestly, but it does not change the same-replica conclusion.
unpaired = sorted(set(b_last) ^ set(a_last))
if unpaired:
    print("NOTE replica switch inside the window (seen on one side only: %s); judged on same-replica "
          "pairs only, cross-replica delta not trusted"
          % ",".join(unpaired))
sys.exit(0)
PY
}
