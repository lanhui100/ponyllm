#!/usr/bin/env bash
# ============================================================
# ha-window.spec.sh —— scripts/lib/ha-window.sh 的负样本自测（不联网）
#
# 契约（机器可判定）：
#   P1  同副本、error 真增长 +2 → FAIL（exit 1），且输出点名该副本
#   P2  同副本、error 不变但有刷新活动 → OK（exit 0）
#   P3  同副本、error 不变且无刷新活动 → WARN 证据不足，仍 exit 0
#   P4  窗口前后无同副本（跨副本采样）→ WARN 跨副本，exit 0（**不得据以 FAIL**）
#   P5  历史累计高值副本单侧出现（191 vs 0 的正 delta 误判场景）→ exit 0
#   P6  任一侧快照为空 → WARN 证据不足，exit 0
#   P7  ha_snapshot_line 解析真实 JSON 形状 → `"<id> <acq> <skip> <err>"`
#   P8  ha_snapshot_line 对非 JSON / 缺 ha_ops → 输出空行（不炸、不伪造）
#
# 手法：直接构造快照文件喂给 ha_window_verdict（纯函数，无网络、无集群）。
# 本 spec 就是「跨副本不得判 FAIL」这条承诺的非零退出判据。
# ============================================================
set -uo pipefail

SPEC="ha-window.spec"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LIB="$ROOT/scripts/lib/ha-window.sh"

[ -f "$LIB" ] || { echo "FAIL: missing $LIB"; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0
ok() { echo "PASS: $1"; pass=$((pass + 1)); }
no() { echo "FAIL: $1"; fail=$((fail + 1)); }

# shellcheck source=../scripts/lib/http-retry.sh
. "$ROOT/scripts/lib/http-retry.sh"
# shellcheck source=../scripts/lib/ha-window.sh
. "$LIB"

# ---- P1：同副本 error 真增长 → 拒绝 ----
printf 'r1 10 2 0\n' > "$TMP/b1"
printf 'r1 12 3 2\n' > "$TMP/a1"
out=$(ha_window_verdict "$TMP/b1" "$TMP/a1" 2>&1); rc=$?
[ "$rc" -eq 1 ] && ok "P1 同副本 error 增长被拒绝（exit 1）" || no "P1 期望 exit 1，实际 $rc"
echo "$out" | grep -q "replica r1" && ok "P1 输出点名副本 r1" || no "P1 输出未点名副本：$out"

# ---- P2：同副本、有活动、error 不变 → 放行 ----
printf 'r1 10 2 5\n' > "$TMP/b2"
printf 'r1 14 2 5\n' > "$TMP/a2"
out=$(ha_window_verdict "$TMP/b2" "$TMP/a2" 2>&1); rc=$?
[ "$rc" -eq 0 ] && ok "P2 同副本 error Δ==0 且有活动 → 放行" || no "P2 期望 exit 0，实际 $rc"
echo "$out" | grep -q "^OK" && ok "P2 判定为 OK" || no "P2 未输出 OK：$out"

# ---- P3：同副本、无活动 → WARN 但放行 ----
printf 'r1 10 2 5\n' > "$TMP/b3"
printf 'r1 10 2 5\n' > "$TMP/a3"
out=$(ha_window_verdict "$TMP/b3" "$TMP/a3" 2>&1); rc=$?
[ "$rc" -eq 0 ] && ok "P3 无活动 → exit 0" || no "P3 期望 exit 0，实际 $rc"
echo "$out" | grep -q "insufficient evidence" && ok "P3 输出证据不足 WARN" || no "P3 缺 WARN：$out"

# ---- P4：窗口前后无同副本 → 不得 FAIL（本次修复的核心承诺） ----
printf 'r1 10 2 0\n' > "$TMP/b4"
printf 'r2 99 9 7\n' > "$TMP/a4"
out=$(ha_window_verdict "$TMP/b4" "$TMP/a4" 2>&1); rc=$?
[ "$rc" -eq 0 ] && ok "P4 跨副本采样不判 FAIL（exit 0）" || no "P4 期望 exit 0，实际 $rc：$out"
echo "$out" | grep -q "cross-replica delta is NOT evidence" && ok "P4 明确说明跨副本不采信" || no "P4 缺跨副本说明：$out"

# ---- P5：复刻 2026-11-24 误判场景（0 -> 191 正 delta，实为换了副本） ----
# before 采到 r2(全 0)，after 采到 r1(历史累计 191)：旧口径判 +191 FAIL，新口径须放行。
printf 'r2 0 0 0\n' > "$TMP/b5"
printf 'r1 32539 18110 191\n' > "$TMP/a5"
out=$(ha_window_verdict "$TMP/b5" "$TMP/a5" 2>&1); rc=$?
[ "$rc" -eq 0 ] && ok "P5 复刻 0->191 副本切换不判 FAIL（exit 0）" || no "P5 期望 exit 0，实际 $rc：$out"

# ---- P5b：同一文件里两侧都出现时，只认同副本配对（不拿 r2 的 0 去比 r1 的 191） ----
printf 'r1 32539 18110 191\nr2 0 0 0\n' > "$TMP/b5b"
printf 'r1 32539 18110 191\nr2 0 0 0\n' > "$TMP/a5b"
out=$(ha_window_verdict "$TMP/b5b" "$TMP/a5b" 2>&1); rc=$?
[ "$rc" -eq 0 ] && ok "P5b 多副本同时可见时按身份配对（exit 0）" || no "P5b 期望 exit 0，实际 $rc：$out"

# ---- P6：快照为空 → WARN 放行（不可达的硬 FAIL 由调用方判定） ----
: > "$TMP/b6"
printf 'r1 1 1 1\n' > "$TMP/a6"
out=$(ha_window_verdict "$TMP/b6" "$TMP/a6" 2>&1); rc=$?
[ "$rc" -eq 0 ] && ok "P6 单侧空快照 → exit 0（WARN）" || no "P6 期望 exit 0，实际 $rc"

# ---- P7：ha_snapshot_line 解析真实 JSON 形状 ----
mkdir -p "$TMP/bin"
cat > "$TMP/bin/curl" <<'MOCK'
#!/usr/bin/env bash
cat <<'JSON'
{"instance_id":"abc-123","total_requests":7,"ha_ops":{"refresh_lock_acquired_total":11,"refresh_lock_skipped_total":2,"refresh_lock_error_total":3,"refresh_persist_failure_total":0,"admin_save_conflicts_total":0,"config_reload_total":4,"refresh_lock_hold_seconds":0}}
JSON
MOCK
chmod +x "$TMP/bin/curl"
line=$(PATH="$TMP/bin:$PATH" ha_snapshot_line "http://example.invalid/v1/telemetry/metrics" 8 1 "")
[ "$line" = "abc-123 11 2 3" ] && ok "P7 解析出 instance_id + 三项计数" || no "P7 解析结果异常：'$line'"

# ---- P8：非 JSON / 缺 ha_ops → 空行（不炸、不伪造数字） ----
cat > "$TMP/bin/curl" <<'MOCK'
#!/usr/bin/env bash
echo '<html>502 Bad Gateway</html>'
MOCK
chmod +x "$TMP/bin/curl"
line=$(PATH="$TMP/bin:$PATH" ha_snapshot_line "http://example.invalid/v1/telemetry/metrics" 8 1 "")
[ -z "$line" ] && ok "P8 非 JSON → 空行（调用方据此判证据不足）" || no "P8 期望空行，实际：'$line'"

cat > "$TMP/bin/curl" <<'MOCK'
#!/usr/bin/env bash
echo '{"instance_id":"abc","total_requests":1}'
MOCK
chmod +x "$TMP/bin/curl"
line=$(PATH="$TMP/bin:$PATH" ha_snapshot_line "http://example.invalid/v1/telemetry/metrics" 8 1 "")
[ -z "$line" ] && ok "P8b 缺 ha_ops → 空行" || no "P8b 期望空行，实际：'$line'"

# ---- P9：判定输出必须是纯 ASCII ----
# 实证 2026-11-24 CI windows-latest：GitHub runner 的 python3 stdout 编码是 cp1252，
# 判定函数 print 中文 → UnicodeEncodeError → traceback 退出 1 → 门禁假红。
# 门禁脚本不得依赖终端编码，这条断言把它钉住。
printf 'r1 10 2 5\n' > "$TMP/b9"
printf 'r1 14 2 5\n' > "$TMP/a9"
out=$(PYTHONIOENCODING=cp1252 ha_window_verdict "$TMP/b9" "$TMP/a9" 2>&1); rc=$?
# 用 python3 判 ASCII（不用 grep -P：BSD grep / git-bash 未必支持 -P）。
if [ "$rc" -eq 0 ] && printf '%s' "$out" | python3 -c "import sys; sys.exit(0 if sys.stdin.read().isascii() else 1)"; then
  ok "P9 判定输出纯 ASCII（cp1252 环境下不炸）"
else
  no "P9 cp1252 下判定异常（rc=$rc）：$out"
fi

echo "---- $SPEC: pass=$pass fail=$fail ----"
[ "$fail" -eq 0 ] || exit 1
