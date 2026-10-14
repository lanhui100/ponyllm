#!/usr/bin/env bash
# ============================================================================
# scripts/zero-downtime-continuity-probe.sh
#
# 零停机部署独立连续性探针（wave-7 protocol-conformance）
#
# 目的：在 CI serialized-rollout 期间（或人工发布窗口内）持续打真实推理流量，
#       独立证明发布全过程"服务不下线"——任何一次 HTTP 非 2xx / 空响应 /
#       JSON 解析失败，都计为一次中断，最终非零退出并打印丢包率。
#
# 与既有门禁的分工：
#   - deploy/prober.py            : 长流连续性单次门禁（CI 每个 role 内调用）
#   - scripts/post-deploy-smoke.sh: 发布后一次性冒烟
#   - 本脚本                       : 发布"期间"的持续采样，产出丢包率物理收据
#
# 两种模式：
#   默认（可用性模式）：只断言 HTTP 200 + 响应体可解析 + 关键字段存在。
#       滚动期间 4 个 role 会先后处于新旧两个镜像，**不可**做线格式断言，
#       否则会把"尚未滚到的旧副本"误判为中断。
#   --strict-shape：额外断言 wave-7 冻结契约的线格式（output_text 而非
#       input_text、anthropic 必带 stop_sequence 等）。仅用于滚动全部完成后的
#       终局一致性验证。
#
# Usage:
#   PONYLLM_API_KEY=sk-... bash scripts/zero-downtime-continuity-probe.sh \
#     --duration 600 --interval 2 --out /tmp/continuity.jsonl
#   PONYLLM_API_KEY=sk-... bash scripts/zero-downtime-continuity-probe.sh \
#     --strict-shape --duration 30
#
# 退出码：0 = 零中断；1 = 存在中断（并打印首个失败的原始响应片段）。
# ============================================================================
set -uo pipefail

BASE="${PONYLLM_BASE_URL:-https://tokens.ponyjob.top}"
KEY="${PONYLLM_API_KEY:-}"
MODEL="${PONYLLM_MODEL:-workbuddy/deepseek-v4.1-flash}"
DURATION=600
INTERVAL=2
OUT=""
STRICT=0

while [ $# -gt 0 ]; do
  case "$1" in
    --duration)     DURATION="${2:?}"; shift 2 ;;
    --interval)     INTERVAL="${2:?}"; shift 2 ;;
    --out)          OUT="${2:?}"; shift 2 ;;
    --base)         BASE="${2:?}"; shift 2 ;;
    --model)        MODEL="${2:?}"; shift 2 ;;
    --strict-shape) STRICT=1; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

command -v curl >/dev/null || { echo "FAIL: curl missing" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 missing" >&2; exit 1; }
[ -n "$KEY" ] || { echo "FAIL: PONYLLM_API_KEY unset" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
[ -n "$OUT" ] || OUT="$TMP/samples.jsonl"
: > "$OUT"

TOTAL=0
FAILS=0
FIRST_FAIL=""
START_TS="$(date +%s)"

check_body() {
  # $1 = body file, $2 = proto
  python3 - "$1" "$2" "$STRICT" <<'PY'
import json, sys
path, proto, strict = sys.argv[1], sys.argv[2], sys.argv[3] == "1"
raw = open(path, encoding="utf-8", errors="replace").read()
d = json.loads(raw)                      # 可用性：必须是合法 JSON
if proto == "responses":
    assert d.get("object") == "response", d.get("object")
    assert isinstance(d.get("output"), list) and d["output"], "output empty"
    if strict:
        assert d.get("status") == "completed", d.get("status")
        for item in d["output"]:
            if item.get("type") == "message":
                for part in item.get("content", []):
                    assert part.get("type") != "input_text", "non-official input_text in output"
                    assert part.get("type") == "output_text", part.get("type")
elif proto == "anthropic":
    assert d.get("type") == "message", d.get("type")
    assert d.get("role") == "assistant", d.get("role")
    assert isinstance(d.get("content"), list), "content not list"
    if strict:
        assert "stop_sequence" in d, "stop_sequence missing"
        assert d["content"], "content empty"
else:
    assert d.get("object") == "chat.completion", d.get("object")
    assert d["choices"][0].get("finish_reason") in ("stop", "length", "tool_calls"), \
        d["choices"][0].get("finish_reason")
PY
}

probe_once() {
  local n="$1" proto="$2" body="$3" url="$4" auth_hdr="$5"
  local code body_file rc ok=1 reason=""
  body_file="$TMP/body.$n"
  code="$(curl -s -m 30 -o "$body_file" -w '%{http_code}' \
    -H "$auth_hdr" -H 'Content-Type: application/json' \
    -X POST "$url" -d "$body" 2>/dev/null)"
  rc=$?
  if [ "$rc" -ne 0 ]; then ok=0; reason="curl_rc=$rc"
  elif [ "$code" != "200" ]; then ok=0; reason="http_$code"
  elif [ ! -s "$body_file" ]; then ok=0; reason="empty_body"
  elif ! check_body "$body_file" "$proto" >/dev/null 2>&1; then ok=0; reason="shape_violation"
  fi

  TOTAL=$((TOTAL+1))
  if [ "$ok" -eq 0 ]; then
    FAILS=$((FAILS+1))
    if [ -z "$FIRST_FAIL" ]; then
      FIRST_FAIL="t=+$(( $(date +%s) - START_TS ))s proto=$proto reason=$reason snippet=$(head -c 300 "$body_file" 2>/dev/null | tr -d '\n')"
    fi
  fi
  printf '{"t":%s,"proto":"%s","ok":%s,"reason":"%s"}\n' \
    "$(( $(date +%s) - START_TS ))" "$proto" "$ok" "$reason" >> "$OUT"
}

echo "== [continuity-probe] base=$BASE model=$MODEL duration=${DURATION}s interval=${INTERVAL}s strict_shape=$STRICT out=$OUT =="
i=0
while [ $(( $(date +%s) - START_TS )) -lt "$DURATION" ]; do
  case $(( i % 3 )) in
    0) probe_once "$i" chat \
         "{\"model\":\"$MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"ping\"}],\"max_tokens\":8}" \
         "$BASE/v1/chat/completions" "Authorization: Bearer $KEY" ;;
    1) probe_once "$i" responses \
         "{\"model\":\"$MODEL\",\"input\":\"ping\",\"max_output_tokens\":16}" \
         "$BASE/v1/responses" "Authorization: Bearer $KEY" ;;
    2) probe_once "$i" anthropic \
         "{\"model\":\"$MODEL\",\"max_tokens\":16,\"messages\":[{\"role\":\"user\",\"content\":\"ping\"}]}" \
         "$BASE/v1/messages" "x-api-key: $KEY" ;;
  esac
  i=$((i+1))
  sleep "$INTERVAL"
done

echo "== [continuity-probe] samples=$TOTAL failures=$FAILS =="
if [ "$FAILS" -ne 0 ]; then
  echo "::error::[ZERO-DOWNTIME-VIOLATION] $FAILS/$TOTAL probes failed"
  echo "first failure: $FIRST_FAIL"
  exit 1
fi
echo "PASS: continuity — $TOTAL/$TOTAL probes succeeded, 0 failures (strict_shape=$STRICT)"
exit 0
