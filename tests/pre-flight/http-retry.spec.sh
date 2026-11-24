#!/usr/bin/env bash
# ============================================================
# http-retry.spec.sh —— scripts/lib/http-retry.sh 的负样本自测（不联网）
#
# 契约（机器可判定）：
#   P1  传输失败两次后成功 → http_code_retry 返回 200（重试真的发生）
#   P2  传输持续失败（curl_rc≠0）→ 返回 000（fail-closed，不静默放行）
#   P3  真实 503 → 立即返回 503 且**只调用一次**（真实响应不重试）
#   P4  真实 404 → 返回 404 且只调用一次（不把 4xx 当传输失败）
#   P5  响应体为空一次后成功 → http_body_retry 返回非空体
#   P6  响应体持续为空 → 返回空（调用方据此 FAIL）
#   P7  token 透传：带 token 时 curl 参数含 Authorization 头
#
# 手法：PATH 注入 mock curl（与 post-deploy-smoke.spec.sh 同范式），
# 调用计数落文件，断言「重试次数」而不只是最终结果。
# ============================================================
set -uo pipefail

SPEC="http-retry.spec"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LIB="$ROOT/scripts/lib/http-retry.sh"

[ -f "$LIB" ] || { echo "FAIL: missing $LIB"; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
MOCK_BIN="$TMP/bin"
mkdir -p "$MOCK_BIN"
CALLS="$TMP/calls"

pass=0
fail=0
ok() { echo "PASS: $1"; pass=$((pass + 1)); }
no() { echo "FAIL: $1"; fail=$((fail + 1)); }

# mock curl：第 N 次调用按 SCRIPT 内容决定行为
#   $1 = 每次调用的行为序列（逗号分隔，元素：ok|empty|rc28|code503|code404）
#   $2 = 成功时的 body
make_mock() {
  local seq="$1" body="${2:-{\"a\":1}}"
  cat > "$MOCK_BIN/curl" <<EOF
#!/usr/bin/env bash
n=\$(cat "$CALLS" 2>/dev/null || echo 0); n=\$((n+1)); echo "\$n" > "$CALLS"
echo "\$*" >> "$TMP/args"
steps=($(printf '%s' "$seq" | sed 's/,/ /g'))
step="\${steps[\$((n-1))]:-\${steps[-1]}}"
# 尊重 -w '%{http_code}' 语义：调用方要状态码时只输出码，不要 body
wants_code=0
for a in "\$@"; do [ "\$a" = "-w" ] && wants_code=1; done
emit() { if [ "\$wants_code" = "1" ]; then printf '%s' "\$1"; else printf '%s' "\$2"; fi; }
case "\$step" in
  ok)      emit 200 '$body'; exit 0 ;;
  empty)   emit 200 ''; exit 0 ;;
  rc28)    printf ''; exit 28 ;;
  code503) emit 503 ''; exit 0 ;;
  code404) emit 404 ''; exit 0 ;;
  *)       printf ''; exit 0 ;;
esac
EOF
  chmod +x "$MOCK_BIN/curl"
  : > "$CALLS"
  : > "$TMP/args"
}

run_lib() { # $1 = 函数名，其余 = 参数
  ( set +u; . "$LIB"; "$@" )
}

# ---------- P1：传输失败两次后成功 ----------
make_mock "rc28,rc28,ok"
out=$(PATH="$MOCK_BIN:$PATH" run_lib http_code_retry "http://x/health" 1 3)
calls=$(cat "$CALLS")
[ "$out" = "200" ] && [ "$calls" = "3" ] \
  && ok "P1 retried transport failures then returned 200 (calls=$calls)" \
  || no "P1 expected 200 after 3 calls, got '$out' calls=$calls"

# ---------- P2：传输持续失败 ----------
make_mock "rc28"
out=$(PATH="$MOCK_BIN:$PATH" run_lib http_code_retry "http://x/health" 1 3)
calls=$(cat "$CALLS")
[ "$out" = "000" ] && [ "$calls" = "3" ] \
  && ok "P2 exhausted retries returned 000 (calls=$calls)" \
  || no "P2 expected 000 after 3 calls, got '$out' calls=$calls"

# ---------- P3：真实 503 不重试 ----------
make_mock "code503"
out=$(PATH="$MOCK_BIN:$PATH" run_lib http_code_retry "http://x/health" 1 3)
calls=$(cat "$CALLS")
[ "$out" = "503" ] && [ "$calls" = "1" ] \
  && ok "P3 real 503 returned immediately without retry (calls=$calls)" \
  || no "P3 expected 503 after 1 call, got '$out' calls=$calls"

# ---------- P4：真实 404 不重试 ----------
make_mock "code404"
out=$(PATH="$MOCK_BIN:$PATH" run_lib http_code_retry "http://x/health" 1 3)
calls=$(cat "$CALLS")
[ "$out" = "404" ] && [ "$calls" = "1" ] \
  && ok "P4 real 404 returned immediately without retry (calls=$calls)" \
  || no "P4 expected 404 after 1 call, got '$out' calls=$calls"

# ---------- P5：空体一次后成功 ----------
make_mock "empty,ok" '{"ha_ops":1}'
out=$(PATH="$MOCK_BIN:$PATH" run_lib http_body_retry "http://x/metrics" 1 3)
calls=$(cat "$CALLS")
[ -n "$out" ] && [ "$calls" = "2" ] \
  && ok "P5 body retried after empty response (calls=$calls)" \
  || no "P5 expected non-empty body after 2 calls, got '$out' calls=$calls"

# ---------- P6：持续空体 ----------
make_mock "empty"
out=$(PATH="$MOCK_BIN:$PATH" run_lib http_body_retry "http://x/metrics" 1 3)
calls=$(cat "$CALLS")
[ -z "$out" ] && [ "$calls" = "3" ] \
  && ok "P6 exhausted body retries returned empty (calls=$calls)" \
  || no "P6 expected empty after 3 calls, got '$out' calls=$calls"

# ---------- P7：token 透传 ----------
make_mock "ok"
out=$(PATH="$MOCK_BIN:$PATH" run_lib http_code_retry "http://x/metrics" 1 1 "tok123")
grep -q 'Authorization: Bearer tok123' "$TMP/args" \
  && ok "P7 auth token forwarded to curl" \
  || no "P7 Authorization header missing (args: $(cat "$TMP/args"))"

echo "----"
echo "summary: $pass passed, $fail failed"
[ "$fail" -eq 0 ] || exit 1
exit 0
