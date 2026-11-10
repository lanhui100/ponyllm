#!/usr/bin/env bash
# ============================================================
# post-deploy-smoke.spec.sh —— 部署冒烟门禁重试与 TRANSIENT/BROKEN 分类自测 (不联网)
#
# 契约：
#   - 自包含：从自身路径解析仓库根，不依赖调用者 cwd
#   - 确定性不联网：通过 PATH 注入 mock curl 模拟各种响应与 curl_rc
#   - 覆盖：
#       ① 429 → 重试 → 200 (最终成功 exit 0)
#       ② 500 持续 → 重试耗尽 exit 1 并输出 SMOKE-TRANSIENT + 包含 body
#       ③ 空响应 + curl_rc=28 (超时) → 重试耗尽 exit 1 并输出 SMOKE-TRANSIENT + curl_rc=28
#       ④ 401 Unauthorized → 不重试直接 exit 1 并输出 SMOKE-BROKEN
# ============================================================
set -euo pipefail

SPEC="post-deploy-smoke.spec"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SMOKE_SCRIPT="$ROOT/scripts/post-deploy-smoke.sh"

[ -f "$SMOKE_SCRIPT" ] || { echo "FAIL: missing $SMOKE_SCRIPT"; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

MOCK_BIN="$TMP/bin"
mkdir -p "$MOCK_BIN"

FAILURES=0
fail() {
  echo "FAIL [$SPEC]: $*" >&2
  FAILURES=1
}

# 辅助函数：生成 mock curl
create_mock_curl() {
  local mock_impl="$1"
  cat << EOF > "$MOCK_BIN/curl"
#!/usr/bin/env bash
args=("\$@")
for ((i=0; i<\${#args[@]}; i++)); do
  if [[ "\${args[i]}" == *"/health"* ]]; then
    echo -n "200"
    exit 0
  fi
  if [[ "\${args[i]}" == *"://"* && "\${args[i]}" == *"/" ]]; then
    echo '<script type="module" crossorigin src="/assets/index-Bn_jvhe1.js"></script>'
    exit 0
  fi
done

OUT_FILE=""
for ((i=0; i<\${#args[@]}; i++)); do
  if [ "\${args[i]}" = "-o" ] && [ \$((i+1)) -lt \${#args[@]} ]; then
    OUT_FILE="\${args[i+1]}"
  fi
done

$mock_impl
EOF
  chmod +x "$MOCK_BIN/curl"
}

# 用例 ①：429 → 重试 → 200 (成功 exit 0)
STATE_FILE="$TMP/case1_count"
create_mock_curl '
COUNT=0
if [ -f "'"$STATE_FILE"'" ]; then
  COUNT="$(cat "'"$STATE_FILE"'")"
fi
COUNT=$((COUNT + 1))
echo "$COUNT" > "'"$STATE_FILE"'"

if [ "$COUNT" -eq 1 ]; then
  echo "{\"error\":{\"message\":\"EndpointTPMExceeded\",\"type\":\"rate_limit_error\"}}" > "$OUT_FILE"
  echo -n "429"
  exit 0
else
  echo "{\"choices\":[{\"finish_reason\":\"stop\",\"message\":{\"content\":\"hello\"}}]}" > "$OUT_FILE"
  echo -n "200"
  exit 0
fi
'

OUT_1="$(PATH="$MOCK_BIN:$PATH" PONYLLM_API_KEY="test_key" bash "$SMOKE_SCRIPT" 2>&1)" && RC_1=0 || RC_1=$?
if [ "$RC_1" -ne 0 ]; then
  fail "Case 1 (429 -> 200) expected exit 0, got $RC_1"
elif ! echo "$OUT_1" | grep -q "PASS: post-deploy smoke OK"; then
  fail "Case 1 output missing PASS mark"
else
  echo "PASS [$SPEC]: Case 1 (429 -> retry -> 200 -> exit 0)"
fi

# 用例 ②：500 持续 → 重试耗尽 exit 1，包含 SMOKE-TRANSIENT 且含 body
create_mock_curl '
echo "{\"error\":\"Internal server error 500\"}" > "$OUT_FILE"
echo -n "500"
exit 0
'

OUT_2="$(PATH="$MOCK_BIN:$PATH" PONYLLM_API_KEY="test_key" bash "$SMOKE_SCRIPT" 2>&1)" && RC_2=0 || RC_2=$?
if [ "$RC_2" -eq 0 ]; then
  fail "Case 2 (continuous 500) expected exit 1, got 0"
elif ! echo "$OUT_2" | grep -q "SMOKE-TRANSIENT: http_code=500"; then
  fail "Case 2 output missing SMOKE-TRANSIENT mark"
elif ! echo "$OUT_2" | grep -q "Internal server error 500"; then
  fail "Case 2 output missing body content in snippet"
else
  echo "PASS [$SPEC]: Case 2 (continuous 500 -> retry exhausted -> SMOKE-TRANSIENT with body)"
fi

# 用例 ③：空响应 + curl_rc=28 (超时) → exit 1 并打印 curl_rc=28
create_mock_curl '
echo -n "000"
exit 28
'

OUT_3="$(PATH="$MOCK_BIN:$PATH" PONYLLM_API_KEY="test_key" bash "$SMOKE_SCRIPT" 2>&1)" && RC_3=0 || RC_3=$?
if [ "$RC_3" -eq 0 ]; then
  fail "Case 3 (curl_rc=28 timeout) expected exit 1, got 0"
elif ! echo "$OUT_3" | grep -q "SMOKE-TRANSIENT: http_code=000 curl_rc=28"; then
  fail "Case 3 output missing SMOKE-TRANSIENT with curl_rc=28"
else
  echo "PASS [$SPEC]: Case 3 (empty response + curl_rc=28 -> SMOKE-TRANSIENT with curl_rc)"
fi

# 用例 ④：401 Unauthorized → 不重试直接 exit 1 并打印 SMOKE-BROKEN
create_mock_curl '
echo "{\"error\":\"Unauthorized\"}" > "$OUT_FILE"
echo -n "401"
exit 0
'

OUT_4="$(PATH="$MOCK_BIN:$PATH" PONYLLM_API_KEY="test_key" bash "$SMOKE_SCRIPT" 2>&1)" && RC_4=0 || RC_4=$?
if [ "$RC_4" -eq 0 ]; then
  fail "Case 4 (401) expected exit 1, got 0"
elif ! echo "$OUT_4" | grep -q "SMOKE-BROKEN: http_code=401"; then
  fail "Case 4 output missing SMOKE-BROKEN mark"
elif echo "$OUT_4" | grep -q "Backing off"; then
  fail "Case 4 should not retry"
else
  echo "PASS [$SPEC]: Case 4 (401 -> direct SMOKE-BROKEN without retry)"
fi

if [ "$FAILURES" -ne 0 ]; then
  echo "[$SPEC] FAILED" >&2
  exit 1
fi

echo "[$SPEC] All negative and retry specs passed! (exit 0)"
exit 0
