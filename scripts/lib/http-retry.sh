#!/usr/bin/env bash
# ============================================================================
# http-retry.sh —— 只读探针的有界重试（部署验收脚本共用）
#
# 为什么存在（实证 2026-11-24）：CI runner 在集群外，验收脚本对公网入口做**单次**
# curl 探测；GitHub runner → 公网链路的偶发中断会把「传输失败」判成「部署失败」：
#   run 45405367795 attempt 1：[3/7] /health=000（同一时刻本机复测连三次 200）
#   run 45405367795 attempt 2：[5/7] metrics 首次快照为空（E0=''，E1 正常）
# 与 2026-10-29「瞬态 vs 破损」同源：**只对传输层失败重试，不对判定放宽**。
#
# 契约：
#   http_code_retry <url> [max_time] [attempts] [auth_token]
#     逐次重试仅当 curl 退出码非 0 或 HTTP 码为 000（传输失败）；
#     拿到任何非 000 码立即返回该码（包括 4xx/5xx——那是真实响应，不重试）。
#     重试耗尽仍为 000 → 打印 `000`，由调用方 FAIL（fail-closed，不静默放行）。
#   http_body_retry <url> [max_time] [attempts] [auth_token]
#     同上口径，但只在**响应体为空**时重试；非空即返回（JSON 合法性由调用方判定）。
#
# 依赖：curl。无副作用、不写盘。
# ============================================================================

# shellcheck shell=bash

http_code_retry() {
  local url="$1" max_time="${2:-8}" attempts="${3:-3}" token="${4:-}"
  local i code
  for i in $(seq 1 "$attempts"); do
    if [ -n "$token" ]; then
      code=$(curl -s -o /dev/null -w '%{http_code}' --max-time "$max_time" \
        -H "Authorization: Bearer $token" "$url" 2>/dev/null) || code='000'
    else
      code=$(curl -s -o /dev/null -w '%{http_code}' --max-time "$max_time" "$url" 2>/dev/null) || code='000'
    fi
    # 任意真实响应（含 4xx/5xx）都不是传输失败：立即返回，交由调用方判定
    if [ -n "$code" ] && [ "$code" != '000' ]; then
      printf '%s\n' "$code"
      return 0
    fi
    [ "$i" -lt "$attempts" ] && sleep 2
  done
  printf '000\n'
  return 0
}

http_body_retry() {
  local url="$1" max_time="${2:-8}" attempts="${3:-3}" token="${4:-}"
  local i body
  for i in $(seq 1 "$attempts"); do
    if [ -n "$token" ]; then
      body=$(curl -s --max-time "$max_time" -H "Authorization: Bearer $token" "$url" 2>/dev/null) || body=''
    else
      body=$(curl -s --max-time "$max_time" "$url" 2>/dev/null) || body=''
    fi
    if [ -n "$body" ]; then
      printf '%s\n' "$body"
      return 0
    fi
    [ "$i" -lt "$attempts" ] && sleep 2
  done
  printf '\n'
  return 0
}
