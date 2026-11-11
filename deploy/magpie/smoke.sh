#!/usr/bin/env bash
# deploy/magpie/smoke.sh — magpie 容器部署的机械验收门禁（非零退出 = 验收失败）
#
# 断言（每条都打了物理凭证：HTTP 状态码 / docker 健康状态）：
#   1. compose 容器存在且 running；镜像内 HEALTHCHECK 为 healthy
#   2. Web UI 不带 key 返回 401（未鉴权不得放行）
#   3. Web UI 带 MAGPIE_WEB_KEY 返回 3xx（key 换成 cookie 后跳转）
#   4. 网关 127.0.0.1:3425/v1/models 不带密钥返回 401
#   5. 网关带 MAGPIE_GATEWAY_KEY 返回 200
#
# 只读：不发任何配置写请求，不打印密钥明文（只打印前缀）。
# 用法：deploy/magpie/smoke.sh            （EXIT 0 = 全绿）
set -uo pipefail

cd "$(dirname "$0")" || exit 1
ENV_FILE=".env"
WEB_PORT="${WEB_PORT:-3430}"
GW_PORT="${GW_PORT:-3425}"
# 探活基址取自 .env 的发布地址：默认回环，设成 tailnet 地址后即验 tailnet 路径。
FAIL=0

fail() { printf 'FAIL  %s\n' "$1"; FAIL=1; }
ok()   { printf 'ok    %s\n' "$1"; }

[ -f "$ENV_FILE" ] || { echo "FAIL 缺少 $ENV_FILE（cp .env.example .env 并填 MAGPIE_WEB_KEY）"; exit 1; }
# shellcheck disable=SC1090
set -a; . "./$ENV_FILE"; set +a
[ -n "${MAGPIE_WEB_KEY:-}" ] || { echo "FAIL MAGPIE_WEB_KEY 为空"; exit 1; }

# 探活基址必须在 source .env 之后才成形：默认回环，设成 tailnet 地址后即验 tailnet 路径。
V4="${MAGPIE_BIND_V4:-127.0.0.1}"
BASE="http://${V4}"
# MagicDNS 名字（取 MAGPIE_PUBLIC_URL 的 host 部分），非回环时额外验一条。
MDNS="$(printf '%s' "${MAGPIE_PUBLIC_URL:-}" | sed -E 's#^https?://([^:/]+).*#\1#')"

# 1) 容器运行 + healthy
STATE="$(docker inspect -f '{{.State.Status}}' magpie 2>/dev/null || echo missing)"
if [ "$STATE" = "running" ]; then
  HEALTH="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' magpie)"
  [ "$HEALTH" = "healthy" ] && ok "container running, health=$HEALTH" \
                            || fail "container running but health=$HEALTH"
else
  fail "container state=$STATE（docker compose up -d）"
fi

code() { local out; out="$(curl -s -o /dev/null -m 5 -w '%{http_code}' "$@" 2>/dev/null)" || { echo 000; return; }; echo "${out:-000}"; }

# 2) Web UI 未鉴权 → 401
C="$(code "${BASE}:${WEB_PORT}/")"
[ "$C" = "401" ] && ok "web 无 key => $C" || fail "web 无 key => $C（期望 401）"

# 3) Web UI 带 key → 3xx
C="$(code "${BASE}:${WEB_PORT}/?k=${MAGPIE_WEB_KEY}")"
case "$C" in 30[0-9]) ok "web 带 key => $C" ;; *) fail "web 带 key => $C（期望 3xx）" ;; esac

# 4) 网关未鉴权 → 401
C="$(code "${BASE}:${GW_PORT}/v1/models")"
[ "$C" = "401" ] && ok "gateway 无 key => $C" || fail "gateway 无 key => $C（期望 401）"

# 5) 网关带网关密钥 → 200
if [ -n "${MAGPIE_GATEWAY_KEY:-}" ]; then
  C="$(code -H "Authorization: Bearer ${MAGPIE_GATEWAY_KEY}" "${BASE}:${GW_PORT}/v1/models")"
  [ "$C" = "200" ] && ok "gateway 带 key => $C" || fail "gateway 带 key => $C（期望 200）"
else
  fail "MAGPIE_GATEWAY_KEY 未设置（docker exec magpie magpie gateway-key add <name>）"
fi

# 6) MagicDNS 名字路径（仅当发布地址不是回环时才有意义）
if [ -n "$MDNS" ] && [ "$MDNS" != "127.0.0.1" ] && [ "$MDNS" != "localhost" ]; then
  C="$(code "http://${MDNS}:${WEB_PORT}/")"
  [ "$C" = "401" ] && ok "magicdns(${MDNS}) 无 key => $C" \
                  || fail "magicdns(${MDNS}) 无 key => $C（期望 401；DNS 不通或端口未在该地址发布）"
  C="$(code -H "Authorization: Bearer ${MAGPIE_GATEWAY_KEY:-}" "http://${MDNS}:${GW_PORT}/v1/models")"
  [ "$C" = "200" ] && ok "magicdns(${MDNS}) 带 key => $C" \
                  || fail "magicdns(${MDNS}) 带 key => $C（期望 200）"
fi

# 7) 回环不应再监听（发布到 tailnet/指定地址时，127.0.0.1 必须关着）
if [ "$V4" != "127.0.0.1" ]; then
  C="$(code "http://127.0.0.1:${GW_PORT}/v1/models")"
  [ "$C" = "000" ] && ok "回环 ${GW_PORT} 已关闭 => 000" \
                   || fail "回环 ${GW_PORT} 仍可连（$C）：发布地址未生效？"
fi

[ "$FAIL" -eq 0 ] && { echo "PASS  magpie 部署验收全绿"; exit 0; }
echo "FAIL  magpie 部署验收未通过"; exit 1
