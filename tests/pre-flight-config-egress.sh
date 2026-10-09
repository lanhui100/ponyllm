#!/usr/bin/env bash
# ============================================================================
# T4b 配置预检（红相先行）：opencode-zen egress 配置"修复后"状态断言
#
# 编写方：L2-T test-agent（冻结，executor-config 只读不可改）
# 用途：task-5 实施（opencode-zen egress_pool 降爆炸半径）的前置/验收门禁；
#       对当前 live 配置必须失败（非零退出 = 红相成立）。
#
# 断言（机器可判定，真实 kubectl + base64 解码）：
#   1) live Secret `ponyllm-live-config` 的 ponyllm.toml 中
#      [providers.opencode-zen] egress_pool == ["direct"]（禁止含 http://pproxy-host:8899
#      类出口条目；当前 live 含 ["direct","http://pproxy-host:8899"] → FAIL=红）；
#   2) muse-spark-1.3-contributor-free 模型级 base_url 仍保留 `pony_` pproxy 路径
#      （该模型必须继续走 pproxy 隧道，egress_pool 变更不得动模型级 base_url）；
#   3) 仓库 deploy/ponyllm-config.example.toml 的 egress_pool 与 live 一致
#      （修复后两者均 == ["direct"]；当前 example 含 5 条 pproxy 条目 → FAIL=红）；
#   4) antigravity 等其余 provider 未被误改（抽查关键字段：antigravity
#      base_url/proxy/default_protocol 与 zen-jev base_url 保持现状）。
#
# 失败语义：任一断言失败 → 打印明确错误消息并最终 exit 1（可判定红相）。
# 凭据脱敏：任何输出不回显 user:pass@ 凭据（proxy 值仅打印 host 段）。
# 环境覆盖：PONYLLM_NAMESPACE（默认 ponyllm）。
# ============================================================================
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NS="${PONYLLM_NAMESPACE:-ponyllm}"
SECRET="ponyllm-live-config"
KEY="ponyllm.toml"
EXAMPLE="$REPO_ROOT/deploy/ponyllm-config.example.toml"

# ── live 配置取数（真实 kubectl + base64，机器断言，缺取数即 fail-closed）────
# jsonpath 键名含 '.' 需转义（否则被当作路径分隔符）。
KEY_JP="$(printf '%s' "$KEY" | sed 's/\./\\./g')"
LIVE_TOML="$(kubectl -n "$NS" get secret "$SECRET" -o jsonpath="{.data.$KEY_JP}" 2>/dev/null | base64 -d 2>/dev/null)"
if [ -z "$LIVE_TOML" ]; then
  echo "FAIL: 无法从 Secret $NS/$SECRET 读取并 base64 解码 $KEY —— kubectl 不可用 / Secret 缺失 / 格式异常（预检 fail-closed）"
  exit 1
fi

# ── TOML 提取助手 ─────────────────────────────────────────────────────────────
# 提取 egress_pool 数组条目（去注释/去尾部逗号/去空白/去引号，逐条一行）。
extract_pool() {
  printf '%s\n' "$1" | awk '
    /^egress_pool[[:space:]]*=/ { in_pool=1; next }
    in_pool && /^[[:space:]]*\]/ { in_pool=0; next }
    in_pool { line=$0; sub(/#.*/,"",line); sub(/^[[:space:]]*/,"",line); sub(/,[[:space:]]*$/,"",line); gsub(/[[:space:]]+/,"",line); gsub(/^"|"$/,"",line); if (length(line)>0) print line }
  '
}

# 提取某 provider 首段内某字段值（去引号/注释/空白）。
section_field() {
  printf '%s\n' "$1" | awk -v sec="$2" -v field="$3" '
    $0 ~ "^\\[" sec "\\]" { in_sec=1; next }
    in_sec && /^\[/ { in_sec=0 }
    in_sec && $0 ~ "^" field "[[:space:]]*=" { sub(/^[^=]*=[[:space:]]*/,""); sub(/#.*/,""); gsub(/^[[:space:]]+|[[:space:]]+$/,""); gsub(/^"|"$/,""); print; exit }
  '
}

# 提取 opencode-zen 下 muse-spark-1.3-contributor-free 模型级 base_url。
muse_base_url() {
  printf '%s\n' "$1" | awk '
    /^\[providers\.opencode-zen\]/ { in_prov=1; next }
    in_prov && /^\[\[providers\.opencode-zen\.model_configs\]\]/ { n=""; b="" }
    in_prov && /^name[[:space:]]*=/ { gsub(/"/,""); split($0,a,"="); gsub(/^[[:space:]]+/,"",a[2]); gsub(/[[:space:]]+$/,"",a[2]); n=a[2] }
    in_prov && /^base_url[[:space:]]*=/ { gsub(/"/,""); split($0,a,"="); gsub(/^[[:space:]]+/,"",a[2]); gsub(/[[:space:]]+$/,"",a[2]); b=a[2] }
    in_prov && /^\[\[providers\.opencode-zen\.keys\]\]/ { exit }
    in_prov && n=="muse-spark-1.3-contributor-free" && length(b)>0 { print b; exit }
  '
}

# 输出条目时脱敏 userinfo 凭据（仅打印 host 段）。
redact_entry() {
  sed -E 's#//[^@/]*@#//<redacted>@#'
}

fail=0

# ── 1) live egress_pool == ["direct"]（无 pproxy 出口）────────────────────────
LIVE_POOL="$(extract_pool "$LIVE_TOML")"
if [ "$(printf '%s\n' "$LIVE_POOL" | grep -c .)" -eq 1 ] && [ "$LIVE_POOL" = "direct" ]; then
  echo "PASS 1: live Secret $NS/$SECRET [providers.opencode-zen] egress_pool == [\"direct\"]"
else
  echo "FAIL 1: live Secret $NS/$SECRET [providers.opencode-zen] egress_pool != [\"direct\"]（修复目标：仅 direct）"
  printf '%s\n' "$LIVE_POOL" | redact_entry | sed 's/^/       当前条目: /'
  if printf '%s\n' "$LIVE_POOL" | grep -qE "pproxy-host|:8899"; then
    echo "        → 违规: 仍含 pproxy 出口条目（http://pproxy-host:8899 类）——降爆炸半径未完成（红相）"
  fi
  fail=1
fi

# ── 2) muse-spark-1.3-contributor-free 模型级 base_url 保留 pony_ pproxy 路径 ─
MUSE_BASE="$(muse_base_url "$LIVE_TOML")"
if printf '%s' "$MUSE_BASE" | grep -q "pony_"; then
  echo "PASS 2: muse-spark-1.3-contributor-free 模型级 base_url 保留 pony_ pproxy 路径（未误改）"
else
  echo "FAIL 2: muse-spark-1.3-contributor-free 模型级 base_url 丢失 pony_ pproxy 路径（模型级 base_url 不得因 egress 变更被改动）：[${MUSE_BASE}]"
  fail=1
fi

# ── 3) 仓库 example egress_pool 与 live 一致（修复后均 == ["direct"]）────────
EXAMPLE_POOL="$(extract_pool "$(cat "$EXAMPLE")")"
if [ "$(printf '%s\n' "$EXAMPLE_POOL" | grep -c .)" -eq 1 ] && [ "$EXAMPLE_POOL" = "direct" ] \
   && [ "$(printf '%s\n' "$LIVE_POOL" | grep -c .)" -eq 1 ] && [ "$LIVE_POOL" = "direct" ]; then
  echo "PASS 3: deploy/ponyllm-config.example.toml 与 live egress_pool 一致（均仅 direct）"
else
  echo "FAIL 3: 仓库 deploy/ponyllm-config.example.toml 与 live egress_pool 不一致（修复目标：均仅 direct）"
  echo "        example 当前条目:"
  printf '%s\n' "$EXAMPLE_POOL" | redact_entry | sed 's/^/           /'
  fail=1
fi

# ── 4) antigravity 等其余 provider 未被误改（抽查关键字段）───────────────────
ANTI_BASE="$(section_field "$LIVE_TOML" 'providers[.]antigravity' 'base_url')"
ANTI_PROXY="$(section_field "$LIVE_TOML" 'providers[.]antigravity' 'proxy')"
ANTI_PROTO="$(section_field "$LIVE_TOML" 'providers[.]antigravity' 'default_protocol')"
if [ "$ANTI_BASE" = "https://daily-cloudcode-pa.googleapis.com" ] \
   && printf '%s' "$ANTI_PROXY" | grep -q "pproxy-host.ponyllm.svc:8899" \
   && [ "$ANTI_PROTO" = "antigravity" ]; then
  echo "PASS 4a: antigravity 关键字段未误改（base_url / proxy 仍走 pproxy-host.ponyllm.svc:8899 CONNECT / default_protocol=antigravity）"
else
  echo "FAIL 4a: antigravity 关键字段被改动（base_url=[${ANTI_BASE}] proxy-host=$(printf '%s' "$ANTI_PROXY" | sed -E 's#^[^@]*@##') proto=[${ANTI_PROTO}]）"
  fail=1
fi
JEV_BASE="$(section_field "$LIVE_TOML" 'providers[.]zen-jev' 'base_url')"
if [ "$JEV_BASE" = "https://opencode.ai/zen/v1" ]; then
  echo "PASS 4b: zen-jev base_url 未误改（https://opencode.ai/zen/v1）"
else
  echo "FAIL 4b: zen-jev base_url 被改动（当前 [${JEV_BASE}]）"
  fail=1
fi

# ── 汇总 ──────────────────────────────────────────────────────────────────────
if [ "$fail" -eq 0 ]; then
  echo "GREEN: opencode-zen egress 配置已收敛到修复后状态（live/example 均仅 direct，muse 隧道路径保留，其余 provider 未误改）"
  exit 0
fi
echo "RED: 一项或多项 egress 配置断言失败（红相成立）——opencode-zen egress_pool 仍含 pproxy 出口，降爆炸半径未完成"
exit 1
