#!/usr/bin/env bash
# pproxy-tencent-restore.sh — 幂等恢复腾讯节点 k3s 出口 pproxy 配置 + 防升级降级守卫
#
# 适用场景:
#   - 腾讯节点重建 / state.db 清空导致 opencode 路由丢失（404）
#   - pproxy upgrade 覆盖了修复版二进制（空 body bug 回归）
#   - config.toml / systemd 环境变量丢失
#
# 用法: 在腾讯节点以 ubuntu 用户执行（需要 passwordless sudo）:
#   bash pproxy-tencent-restore.sh
# 任意一步失败以非零退出，可重复执行（幂等）。
set -euo pipefail

EXPECTED_SHA="a5ed9904bc201ac364daac16606fbddac80ddebb29ba3cc29a4b48823cfebae6"
BIN="/home/ubuntu/.local/bin/pproxy"
ADMIN="http://127.0.0.1:8900"
CONFIG="/home/ubuntu/.pony/config.toml"
UNIT="/etc/systemd/system/pproxy.service"
ROUTE_NAME="opencode"
TARGET_HOST="opencode.ai"
OVERRIDE="vercel"

echo "[1/5] 校验 pproxy 二进制（防 upgrade 降级，期望修复版 ≥ commit 70341aa）..."
if [ ! -f "$BIN" ]; then
  echo "ERROR: $BIN 不存在。需先部署修复版: 在 pproxy 工作区 cargo build --release -p pproxy-cli，scp 到本机。"
  exit 1
fi
ACTUAL=$(sha256sum "$BIN" | awk '{print $1}')
if [ "$ACTUAL" != "$EXPECTED_SHA" ]; then
  echo "ERROR: 二进制与修复版不符（很可能被 'pproxy upgrade' 覆盖为未修复的发布版）。"
  echo "  期望 sha256: $EXPECTED_SHA"
  echo "  实际 sha256: $ACTUAL"
  echo "  处置: pproxy 工作区构建修复版后覆盖安装: "
  echo "    cd <pproxy-repo> && cargo build --release -p pproxy-cli"
  echo "    scp target/release/pproxy tencent:/home/ubuntu/.local/bin/pproxy.new"
  echo "    ssh tencent 'mv pproxy.new pproxy && sudo systemctl restart pproxy'"
  echo "  在发布修复版 CLI（≥ cli-v0.3.56）前，勿再执行 pproxy upgrade。"
  exit 1
fi
echo "  OK"

echo "[2/5] 确保 opencode 路由存在..."
AT="${PPROXY_ADMIN_TOKEN:-}"
if [ -z "$AT" ]; then
  if [ -f "$CONFIG" ]; then
    AT=$(awk -F'=' '/^admin_token/ {gsub(/[ "]/, "", $2); print $2}' "$CONFIG" || true)
  fi
fi
if [ -z "$AT" ]; then
  echo "ERROR: 未设置 PPROXY_ADMIN_TOKEN 且无法从 $CONFIG 获取 admin_token。"
  exit 1
fi

EXIST=$(curl -s -m 5 -H "Authorization: Bearer $AT" "$ADMIN/api/routes" | grep -c "\"name\":\"$ROUTE_NAME\"") || true
if [ "$EXIST" = "0" ]; then
  CODE=$(curl -s -m 8 -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $AT" \
    -H 'Content-Type: application/json' \
    -d "{\"name\":\"$ROUTE_NAME\",\"target_host\":\"$TARGET_HOST\",\"override_upstream\":\"$OVERRIDE\"}" \
    "$ADMIN/api/routes")
  [ "$CODE" = "201" ] || { echo "ERROR: 路由注册失败 HTTP $CODE"; exit 1; }
  echo "  路由已注册"
else
  echo "  已存在"
fi

echo "[3/5] 确保 config.toml 含 proxy_secret（vercel 边缘所需）..."
if ! grep -q '^proxy_secret' "$CONFIG"; then
  echo "ERROR: config.toml 缺 proxy_secret。请手动补上（与 edge/vedge 共享的 PROXY_SECRET 同值），然后重跑本脚本。"
  exit 1
fi
echo "  OK"

echo "[4/5] 确保 systemd 单元含边缘 URL 环境变量..."
if ! grep -q 'PPROXY_EDGE_URL' "$UNIT"; then
  sudo sed -i '/Environment=PPROXY_TUNNEL_TOKEN=/a Environment=PPROXY_EDGE_URL=https://edge.ponygo.fun\nEnvironment=PPROXY_VERCEL_URL=https://vedge.ponygo.fun/api/proxy' "$UNIT"
  sudo systemctl daemon-reload
  sudo systemctl restart pproxy
  sleep 6
  echo "  已追加并重启"
else
  echo "  已存在"
fi

echo "[5/5] 健康检查（route-first 路径）..."
CODE=$(curl -s -m 10 -o /dev/null -w '%{http_code}' "http://127.0.0.1:18899/opencode/zen/v1/models")
[ "$CODE" = "200" ] || { echo "ERROR: 健康检查失败 HTTP $CODE（服务未就绪或路由/边缘仍缺）"; exit 1; }
echo "  200 OK — 腾讯节点出口已就绪"
echo "完成。"
